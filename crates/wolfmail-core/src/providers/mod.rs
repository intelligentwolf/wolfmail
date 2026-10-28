//! Provider APIs: accounts signed in with Google or Microsoft sync through the Gmail API / Microsoft Graph instead
//! of IMAP/SMTP, because the OAuth apps WolfMail shares with IntelligentWolf are approved for API scopes only
//! (Source: ../intelligentwolf/php-api/lib/gmail.php:28-50 GMAIL_SCOPES = gmail.modify, gmail.send,
//! gmail.settings.basic …; php-api/lib/microsoft.php:20-29 MS_SCOPES = Mail.Read, Mail.Send, Mail.ReadWrite,
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

/// Retry budget for throttling / transient server errors.
const MAX_ATTEMPTS: u32 = 4;
const MAX_WAIT: Duration = Duration::from_secs(30);

/// An HTTP status the caller wants to handle itself (Gmail history 404 → full sync; Graph delta 410 → reset).
#[derive(Debug)]
pub enum Fetch<T> {
    Ok(T),
    Status(u16),
}

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    token: String,
    base: String,
    graph: bool,
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
    pub fn new(http: &reqwest::Client, token: String, base: &str, graph: bool) -> Self {
        ApiClient { http: http.clone(), token, base: base.trim_end_matches('/').to_string(), graph }
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
                let wait = retry_after.map(Duration::from_secs).unwrap_or_else(|| Duration::from_secs(1 << (attempt - 1))).min(MAX_WAIT);
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
