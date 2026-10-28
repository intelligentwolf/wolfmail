//! Provider APIs: accounts signed in with Google or Microsoft sync through the Gmail API / Microsoft Graph instead
//! of IMAP/SMTP, because the OAuth apps WolfMail shares with IntelligentWolf are approved for API scopes only
//! (Source: IntelligentWolf's own Google app scopes = gmail.modify, gmail.send,
//! gmail.settings.basic …; IntelligentWolf's Microsoft scopes = Mail.Read, Mail.Send, Mail.ReadWrite,
//! User.Read, offline_access …). Neither includes `https://mail.google.com/` nor `IMAP.AccessAsUser.All`, which
//! IMAP/SMTP with OAuth would need.
//!
//! Password accounts (including Gmail / Outlook with an app password) stay on IMAP.
pub mod gmail;
pub mod graph;
pub mod store;

use crate::error::{ApiError, ErrorKind, Result};
use crate::model::{Account, AuthKind};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// How an account talks to its server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Imap,
    Gmail,
    Graph,
}

/// OAuth ⇒ provider API (the only scopes WolfMail's OAuth apps hold); password ⇒ IMAP/SMTP.
pub fn backend_for_auth(auth: AuthKind) -> Backend {
    match auth {
        AuthKind::OauthGoogle => Backend::Gmail,
        AuthKind::OauthMicrosoft => Backend::Graph,
        AuthKind::Password => Backend::Imap,
    }
}

pub fn backend(acct: &Account) -> Backend {
    backend_for_auth(acct.imap.auth)
}

/// API roots. Production values below; tests point them at a local mock server.
#[derive(Debug, Clone)]
pub struct ApiBases {
    /// Source: Gmail API reference — every method is under `https://gmail.googleapis.com/gmail/v1/users/{userId}`.
    pub gmail: String,
    /// Source: Microsoft Graph v1.0 reference — `https://graph.microsoft.com/v1.0`.
    pub graph: String,
}

impl Default for ApiBases {
    fn default() -> Self {
        ApiBases { gmail: "https://gmail.googleapis.com/gmail/v1".into(), graph: "https://graph.microsoft.com/v1.0".into() }
    }
}

/// Retry budget for throttling / transient server errors. Backoff 1+2+4+8+16+32+60 s ≈ 2 minutes: longer than
/// Gmail's one-minute quota window, so a request that met a spent quota waits it out instead of failing.
const MAX_ATTEMPTS: u32 = 8;
const MAX_WAIT: Duration = Duration::from_secs(60);

/// An HTTP status the caller wants to handle itself (Gmail history 404 → full sync; Graph delta 410 → reset).
#[derive(Debug)]
pub enum Fetch<T> {
    Ok(T),
    Status(u16),
}

/// Gmail requests go through one queue per Google user, spaced by what each costs. Source: Gmail API "Usage
/// limits" (developers.google.com/workspace/gmail/api/reference/quota, read 28 Oct 2026): 6,000 quota units per
/// user per minute, with a per-method cost (gmail_cost). The earlier pacer assumed 250 units/second and 5 units per
/// messages.get (now 20, or 60 for raw/full), so a sync spent the minute's quota in seconds and a delete straight
/// after got 403 rateLimitExceeded until the retries ran out (Paul, 28 Oct 2026).
/// 90 units/s with at most 500 saved up: any 60 s window then holds at most 500 + 60 × 90 = 5,900 units.
const GMAIL_UNITS_PER_SEC: f64 = 90.0;
const GMAIL_BURST_UNITS: f64 = 500.0;
/// A method the quota page doesn't list (users.labels.patch) is charged as the dearest listed one (drafts.send,
/// messages.send: 100), so an unknown cost can only slow the queue down, never overrun the quota.
const GMAIL_UNLISTED_COST: f64 = 100.0;

/// Quota units for one Gmail API request. Source: the Gmail "Usage limits" per-method table cited above.
fn gmail_cost(method: &reqwest::Method, url: &reqwest::Url) -> f64 {
    use reqwest::Method as M;
    let path = url.path();
    let rest = path.find("/users/me/").map(|i| &path[i + "/users/me/".len()..]).unwrap_or("");
    let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
    let heavy_format = url.query_pairs().any(|(k, v)| k == "format" && matches!(v.as_ref(), "full" | "raw" | "html"));
    match (method, parts.as_slice()) {
        (&M::GET, ["profile"]) => 1.0,                                          // getProfile
        (&M::GET, ["history"]) => 2.0,                                          // history.list
        (&M::GET, ["labels"]) | (&M::GET, ["labels", _]) => 1.0,                // labels.list / labels.get
        (&M::POST, ["labels"]) => 5.0,                                          // labels.create
        (&M::PUT, ["labels", _]) | (&M::DELETE, ["labels", _]) => 5.0,          // labels.update / labels.delete
        (&M::GET, ["messages"]) => 5.0,                                         // messages.list
        (&M::POST, ["messages"]) => 25.0,                                       // messages.insert
        (&M::GET, ["messages", _]) => if heavy_format { 60.0 } else { 20.0 },   // messages.get
        (&M::POST, ["messages", "batchModify"]) => 50.0,                        // messages.batchModify
        (&M::POST, ["messages", "send"]) => 100.0,                              // messages.send
        (&M::POST, ["messages", _, "modify"]) => 5.0,                           // messages.modify
        (&M::POST, ["messages", _, "trash"]) => 20.0,                           // messages.trash
        (&M::POST, ["messages", _, "untrash"]) => 5.0,                          // messages.untrash
        (&M::GET, ["drafts"]) => 5.0,                                           // drafts.list
        (&M::POST, ["drafts"]) => 10.0,                                         // drafts.create
        (&M::GET, ["drafts", _]) => if heavy_format { 60.0 } else { 20.0 },     // drafts.get
        (&M::PUT, ["drafts", _]) => 15.0,                                       // drafts.update
        (&M::DELETE, ["drafts", _]) => 10.0,                                    // drafts.delete
        (&M::POST, ["drafts", "send"]) => 100.0,                                // drafts.send
        _ => GMAIL_UNLISTED_COST,
    }
}

struct Bucket {
    units: f64,
    at: tokio::time::Instant,
}

static GMAIL_QUEUES: std::sync::LazyLock<parking_lot::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<Bucket>>>>> =
    std::sync::LazyLock::new(Default::default);

fn gmail_queue(user: &str) -> Arc<tokio::sync::Mutex<Bucket>> {
    let mut all = GMAIL_QUEUES.lock();
    Arc::clone(all.entry(user.to_ascii_lowercase()).or_insert_with(|| {
        Arc::new(tokio::sync::Mutex::new(Bucket { units: GMAIL_BURST_UNITS, at: tokio::time::Instant::now() }))
    }))
}

/// Wait for this request's turn. The tokio mutex hands out turns in arrival order and is held while waiting, so
/// requests leave the queue one by one at the quota rate.
async fn gmail_take(user: &str, cost: f64) {
    let q = gmail_queue(user);
    let mut b = q.lock().await;
    let now = tokio::time::Instant::now();
    b.units = (b.units + now.duration_since(b.at).as_secs_f64() * GMAIL_UNITS_PER_SEC).min(GMAIL_BURST_UNITS);
    b.at = now;
    if b.units < cost {
        tokio::time::sleep(Duration::from_secs_f64((cost - b.units) / GMAIL_UNITS_PER_SEC)).await;
        b.units = cost;
        b.at = tokio::time::Instant::now();
    }
    b.units -= cost;
}

/// Google said slow down anyway (another WolfMail on the same account, or quota spent before a restart): empty the
/// saved-up units so the queue runs at the steady rate from here.
async fn gmail_drain(user: &str) {
    let q = gmail_queue(user);
    let mut b = q.lock().await;
    b.units = 0.0;
    b.at = tokio::time::Instant::now();
}

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    token: String,
    base: String,
    graph: bool,
    /// The Google/Microsoft user the token belongs to; Gmail's quota queue is per user.
    user: String,
}

fn error_message(body: &[u8]) -> String {
    // Gmail: {"error":{"code":403,"message":"…","errors":[{"reason":"rateLimitExceeded"}]}}
    // Graph: {"error":{"code":"ErrorItemNotFound","message":"…"}}
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message").and_then(|m| m.as_str()).map(str::to_string))
        .unwrap_or_else(|| String::from_utf8_lossy(&body[..body.len().min(300)]).to_string())
}

fn is_rate_limit_403(body: &[u8]) -> bool {
    // Source: Gmail API "Resolve errors" — 403 userRateLimitExceeded / rateLimitExceeded are retried with backoff.
    let s = String::from_utf8_lossy(body);
    s.contains("userRateLimitExceeded") || s.contains("rateLimitExceeded")
}

impl ApiClient {
    pub fn new(http: &reqwest::Client, token: String, base: &str, graph: bool, user: &str) -> Self {
        ApiClient { http: http.clone(), token, base: base.trim_end_matches('/').to_string(), graph, user: user.to_string() }
    }

    pub fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{}", self.base, path)
        }
    }

    /// Send with retries. `build` makes a fresh request each attempt. Returns the response or the status the
    /// caller asked to see (`pass_through`).
    async fn send(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
        pass_through: &[u16],
    ) -> Result<Fetch<reqwest::Response>> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            if !self.graph {
                let cost = build().build().map(|r| gmail_cost(r.method(), r.url())).unwrap_or(GMAIL_UNLISTED_COST);
                gmail_take(&self.user, cost).await;
            }
            let mut req = build().bearer_auth(&self.token);
            if self.graph {
                // Source: Graph "Obtain immutable identifiers" — the header applies only to the request it is on,
                // so it goes on every request; message ids then survive moves between folders.
                req = req.header("Prefer", "IdType=\"ImmutableId\"");
            }
            let resp = req.send().await?;
            let status = resp.status().as_u16();
            if resp.status().is_success() {
                return Ok(Fetch::Ok(resp));
            }
            if pass_through.contains(&status) {
                return Ok(Fetch::Status(status));
            }
            let retry_after = resp
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok());
            let body = resp.bytes().await.unwrap_or_default();
            // Source: Gmail "Resolve errors" (429, 403 rate limits, 500/502/503/504 → exponential backoff starting
            // at ≥1 s); Graph "Throttling guidance" (429 + Retry-After seconds; exponential backoff if absent).
            let transient = matches!(status, 429 | 500 | 502 | 503 | 504) || (status == 403 && is_rate_limit_403(&body));
            if transient && attempt < MAX_ATTEMPTS {
                if !self.graph && matches!(status, 429 | 403) {
                    gmail_drain(&self.user).await;
                }
                // Source: Gmail "Usage limits" — exponential backoff min(2^n + random ms ≤ 1000, maximum backoff).
                let jitter = Duration::from_millis(u64::from(rand::random::<u16>() % 1000));
                let wait = retry_after.map(Duration::from_secs).unwrap_or_else(|| Duration::from_secs(1 << (attempt - 1)) + jitter).min(MAX_WAIT);
                tokio::time::sleep(wait).await;
                continue;
            }
            let msg = error_message(&body);
            return Err(match status {
                401 => ApiError::auth(format!("The provider refused WolfMail's sign-in ({msg}). Sign in again.")),
                403 if !transient => ApiError::new(ErrorKind::Auth, format!("The provider refused access: {msg}")),
                404 => ApiError::not_found("The item on the server (it may have been deleted elsewhere)"),
                _ if transient => ApiError::network(format!("The provider is busy right now ({status}). WolfMail will try again.")),
                _ => ApiError::server(format!("The provider answered {status}: {msg}")),
            });
        }
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http.request(method, self.url(path))
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        match self.get_json_status(path, query, &[], &[]).await? {
            Fetch::Ok(v) => Ok(v),
            Fetch::Status(s) => Err(ApiError::server(format!("Unexpected status {s}"))),
        }
    }

    /// GET returning `Fetch::Status` for the listed statuses instead of an error.
    pub async fn get_json_status<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        extra_prefer: &[&str],
        pass_through: &[u16],
    ) -> Result<Fetch<T>> {
        let r = self
            .send(
                || {
                    let mut b = self.req(reqwest::Method::GET, path).query(query);
                    for p in extra_prefer {
                        b = b.header("Prefer", *p);
                    }
                    b
                },
                pass_through,
            )
            .await?;
        match r {
            Fetch::Ok(resp) => {
                let bytes = resp.bytes().await?;
                Ok(Fetch::Ok(serde_json::from_slice(&bytes).map_err(|e| ApiError::server(format!("Unexpected answer from the provider: {e}")))?))
            }
            Fetch::Status(s) => Ok(Fetch::Status(s)),
        }
    }

    pub async fn get_bytes(&self, path: &str, query: &[(&str, String)]) -> Result<Vec<u8>> {
        match self.send(|| self.req(reqwest::Method::GET, path).query(query), &[]).await? {
            Fetch::Ok(resp) => Ok(resp.bytes().await?.to_vec()),
            Fetch::Status(s) => Err(ApiError::server(format!("Unexpected status {s}"))),
        }
    }

    async fn json_body(&self, method: reqwest::Method, path: &str, body: &Value) -> Result<Value> {
        match self.send(|| self.req(method.clone(), path).json(body), &[]).await? {
            Fetch::Ok(resp) => {
                let bytes = resp.bytes().await?;
                if bytes.is_empty() {
                    return Ok(Value::Null);
                }
                Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
            }
            Fetch::Status(s) => Err(ApiError::server(format!("Unexpected status {s}"))),
        }
    }

    pub async fn post_json(&self, path: &str, body: &Value) -> Result<Value> {
        self.json_body(reqwest::Method::POST, path, body).await
    }
    pub async fn put_json(&self, path: &str, body: &Value) -> Result<Value> {
        self.json_body(reqwest::Method::PUT, path, body).await
    }
    pub async fn patch_json(&self, path: &str, body: &Value) -> Result<Value> {
        self.json_body(reqwest::Method::PATCH, path, body).await
    }

    /// POST with an empty body (Gmail trash/untrash, Graph permanentDelete).
    pub async fn post_empty(&self, path: &str) -> Result<Value> {
        match self.send(|| self.req(reqwest::Method::POST, path).header("Content-Length", "0"), &[]).await? {
            Fetch::Ok(resp) => {
                let bytes = resp.bytes().await?;
                Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
            }
            Fetch::Status(s) => Err(ApiError::server(format!("Unexpected status {s}"))),
        }
    }

    /// POST a text/plain body (Graph MIME uploads are base64 text — Source: Graph sendMail / Create message,
    /// "Use `text/plain` for MIME content").
    pub async fn post_text(&self, path: &str, text: String) -> Result<Value> {
        match self.send(|| self.req(reqwest::Method::POST, path).header("Content-Type", "text/plain").body(text.clone()), &[]).await? {
            Fetch::Ok(resp) => {
                let bytes = resp.bytes().await?;
                Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
            }
            Fetch::Status(s) => Err(ApiError::server(format!("Unexpected status {s}"))),
        }
    }

    pub async fn delete(&self, path: &str) -> Result<()> {
        match self.send(|| self.req(reqwest::Method::DELETE, path), &[404]).await? {
            // Already gone is what a delete wants.
            Fetch::Ok(_) | Fetch::Status(404) => Ok(()),
            Fetch::Status(s) => Err(ApiError::server(format!("Unexpected status {s}"))),
        }
    }
}

/// Run `f` over `items` with at most `n` in flight, keeping input order in the output.
pub async fn bounded<T, R, F, Fut>(items: Vec<T>, n: usize, f: F) -> Vec<R>
where
    F: Fn(T) -> Fut,
    Fut: std::future::Future<Output = R>,
{
    use futures::stream::{self, StreamExt};
    stream::iter(items.into_iter().map(f)).buffered(n.max(1)).collect().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost(m: reqwest::Method, u: &str) -> f64 {
        gmail_cost(&m, &reqwest::Url::parse(&format!("https://gmail.googleapis.com/gmail/v1/users/me/{u}")).unwrap())
    }

    #[test]
    fn gmail_costs_follow_the_quota_table() {
        use reqwest::Method as M;
        assert_eq!(cost(M::GET, "messages/abc?format=metadata"), 20.0);
        assert_eq!(cost(M::GET, "messages/abc?format=minimal"), 20.0);
        assert_eq!(cost(M::GET, "messages/abc?format=raw"), 60.0);
        assert_eq!(cost(M::POST, "messages/batchModify"), 50.0);
        assert_eq!(cost(M::POST, "messages/abc/modify"), 5.0);
        assert_eq!(cost(M::POST, "messages/abc/trash"), 20.0);
        assert_eq!(cost(M::POST, "messages/abc/untrash"), 5.0);
        assert_eq!(cost(M::POST, "messages/send"), 100.0);
        assert_eq!(cost(M::POST, "messages"), 25.0);
        assert_eq!(cost(M::GET, "messages?q=x"), 5.0);
        assert_eq!(cost(M::GET, "history?startHistoryId=1"), 2.0);
        assert_eq!(cost(M::GET, "labels"), 1.0);
        assert_eq!(cost(M::GET, "profile"), 1.0);
        assert_eq!(cost(M::PUT, "drafts/d1"), 15.0);
        assert_eq!(cost(M::POST, "drafts"), 10.0);
        assert_eq!(cost(M::DELETE, "drafts/d1"), 10.0);
        // users.labels.patch is not on the quota page: charged as the dearest listed method.
        assert_eq!(cost(M::PATCH, "labels/L1"), GMAIL_UNLISTED_COST);
    }

    #[tokio::test(start_paused = true)]
    async fn gmail_queue_stays_under_the_per_minute_quota() {
        // 400 metadata gets (20 units each = 8,000 units) queued at once for one user: no 60 s window may exceed
        // Google's 6,000 units.
        let user = "quota-test@example.com";
        let start = tokio::time::Instant::now();
        let mut done = Vec::new();
        for _ in 0..400 {
            gmail_take(user, 20.0).await;
            done.push(start.elapsed().as_secs_f64());
        }
        for (i, t) in done.iter().enumerate() {
            let in_window = done[i..].iter().take_while(|u| **u < t + 60.0).count() as f64 * 20.0;
            assert!(in_window <= 6000.0, "{in_window} units in the minute from {t:.1}s");
        }
        // Another user has a queue of their own.
        let other = tokio::time::Instant::now();
        gmail_take("someone-else@example.com", 20.0).await;
        assert!(other.elapsed() < Duration::from_millis(1));
    }

    #[test]
    fn backend_follows_auth() {
        assert_eq!(backend_for_auth(AuthKind::OauthGoogle), Backend::Gmail);
        assert_eq!(backend_for_auth(AuthKind::OauthMicrosoft), Backend::Graph);
        assert_eq!(backend_for_auth(AuthKind::Password), Backend::Imap);
    }

    #[test]
    fn error_bodies() {
        assert_eq!(error_message(br#"{"error":{"code":"ErrorItemNotFound","message":"Not found."}}"#), "Not found.");
        assert!(is_rate_limit_403(br#"{"error":{"code":403,"errors":[{"reason":"userRateLimitExceeded"}]}}"#));
        assert!(!is_rate_limit_403(br#"{"error":{"code":403,"errors":[{"reason":"insufficientPermissions"}]}}"#));
    }
}
