//! IntelligentWolf client. Pairing (device-code flow) and the `/wolfmail/*` device routes.
// Source: docs/IW-INTEGRATION.md — every path, body and response shape below is taken from that file.
use crate::error::{ApiError, ErrorKind, Result};
use crate::secrets::{SecretStore, IW_DEVICE_KEY};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IwUser {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IwOrg {
    pub id: Value,
    pub name: String,
}

/// What WolfMail keeps in the keychain once paired.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IwDevice {
    pub device_token: String,
    pub user: IwUser,
    pub org: IwOrg,
    pub base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IwStatus {
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<IwUser>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<IwOrg>,
    pub base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairStart {
    pub pair_id: String,
    pub poll_secret: String,
    pub code: String,
    pub verify_url: String,
    pub expires_at: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairStartPublic {
    pub code: String,
    pub verify_url: String,
    pub expires_at: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairPoll {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<IwUser>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<IwOrg>,
}

pub fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn join(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

/// Base URL must be https (or http to this computer, for the test server). Source: docs/IW-INTEGRATION.md
/// "Base URL … overridable"; the device token must never travel in clear text.
pub fn validate_base(base: &str) -> Result<String> {
    let u = url::Url::parse(base.trim()).map_err(|_| ApiError::invalid("iwBaseUrl", "Enter a full address, like https://intelligentwolf.com/api."))?;
    let local = matches!(u.host_str(), Some("127.0.0.1") | Some("localhost") | Some("[::1]"));
    if u.scheme() != "https" && !(u.scheme() == "http" && local) {
        return Err(ApiError::invalid("iwBaseUrl", "The IntelligentWolf address must start with https://."));
    }
    Ok(base.trim().trim_end_matches('/').to_string())
}

async fn read_json(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let body = resp.bytes().await?;
    let v: Value = if body.is_empty() { Value::Null } else { serde_json::from_slice(&body).unwrap_or(Value::Null) };
    if status.is_success() {
        return Ok(v);
    }
    let server_msg = v.get("error").and_then(|e| e.as_str()).or_else(|| v.get("message").and_then(|e| e.as_str())).unwrap_or("");
    Err(match status.as_u16() {
        401 | 403 => ApiError::new(ErrorKind::Auth, "IntelligentWolf no longer accepts this computer. Connect WolfMail again from Settings → IntelligentWolf."),
        402 => ApiError::new(ErrorKind::Server, "You are out of IntelligentWolf credits. Top up in IntelligentWolf → Billing to use AI features."),
        404 => ApiError::not_found("That IntelligentWolf feature"),
        429 => ApiError::server("IntelligentWolf is busy. Try again in a minute."),
        _ => ApiError::server(format!("IntelligentWolf returned an error ({status}){}", if server_msg.is_empty() { String::new() } else { format!(": {server_msg}") })),
    })
}

pub struct IwClient<'a> {
    pub http: &'a reqwest::Client,
    pub secrets: &'a dyn SecretStore,
}

impl IwClient<'_> {
    pub fn device(&self) -> Result<Option<IwDevice>> {
        match self.secrets.get(IW_DEVICE_KEY)? {
            Some(s) => Ok(serde_json::from_str(&s).ok()),
            None => Ok(None),
        }
    }

    fn require(&self) -> Result<IwDevice> {
        self.device()?.ok_or_else(|| ApiError::new(ErrorKind::Auth, "Connect IntelligentWolf first (Settings → IntelligentWolf)."))
    }

    pub fn status(&self, default_base: &str) -> Result<IwStatus> {
        Ok(match self.device()? {
            Some(d) => IwStatus { connected: true, user: Some(d.user), org: Some(d.org), base_url: d.base_url },
            None => IwStatus { connected: false, user: None, org: None, base_url: default_base.to_string() },
        })
    }

    pub async fn pair_start(&self, base: &str, device_name: &str, app_version: &str) -> Result<PairStart> {
        let base = validate_base(base)?;
        let resp = self
            .http
            .post(join(&base, "/wolfmail/pair/start"))
            .json(&json!({ "deviceName": device_name, "platform": platform(), "appVersion": app_version }))
            .send()
            .await?;
        let v = read_json(resp).await?;
        serde_json::from_value(v).map_err(|e| ApiError::server(format!("Unexpected pairing reply from IntelligentWolf: {e}")))
    }

    /// One poll. On "done" the device token is stored and the pairing is complete.
    pub async fn pair_poll(&self, base: &str, pair: &PairStart) -> Result<String> {
        let base = validate_base(base)?;
        let resp = self
            .http
            .post(join(&base, "/wolfmail/pair/poll"))
            .json(&json!({ "pairId": pair.pair_id, "pollSecret": pair.poll_secret }))
            .send()
            .await?;
        let p: PairPoll = serde_json::from_value(read_json(resp).await?).map_err(|e| ApiError::server(format!("Unexpected pairing reply: {e}")))?;
        if p.state == "done" {
            let (Some(token), Some(user), Some(org)) = (p.device_token, p.user, p.org) else {
                return Err(ApiError::server("IntelligentWolf finished pairing without sending the device details."));
            };
            let dev = IwDevice { device_token: token, user, org, base_url: base };
            self.secrets.set(IW_DEVICE_KEY, &serde_json::to_string(&dev)?)?;
        }
        Ok(p.state)
    }

    async fn call(&self, method: reqwest::Method, path: &str, query: &[(&str, &str)], body: Option<Value>) -> Result<Value> {
        let d = self.require()?;
        let mut req = self.http.request(method, join(&d.base_url, path)).bearer_auth(&d.device_token).query(query);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await?;
        let r = read_json(resp).await;
        if let Err(e) = &r {
            if e.kind == ErrorKind::Auth {
                // The server revoked this device; forget it so the UI shows "not connected".
                let _ = self.secrets.delete(IW_DEVICE_KEY);
            }
        }
        r
    }

    pub async fn me(&self) -> Result<Value> {
        self.call(reqwest::Method::GET, "/wolfmail/me", &[], None).await
    }

    pub async fn person(&self, email: &str) -> Result<Value> {
        let v = self.call(reqwest::Method::GET, "/wolfmail/person", &[("email", email)], None).await?;
        // `{ kind: "none" }` means no match → the UI contract is `null`.
        if v.get("kind").and_then(|k| k.as_str()) == Some("none") {
            return Ok(Value::Null);
        }
        Ok(v)
    }

    pub async fn ai(&self, body: Value) -> Result<Value> {
        self.call(reqwest::Method::POST, "/wolfmail/ai", &[], Some(body)).await
    }

    pub async fn log_email(&self, body: Value) -> Result<Value> {
        self.call(reqwest::Method::POST, "/wolfmail/log-email", &[], Some(body)).await
    }

    pub async fn create_task(&self, body: Value) -> Result<Value> {
        self.call(reqwest::Method::POST, "/wolfmail/tasks", &[], Some(body)).await
    }

    pub async fn booking_links(&self) -> Result<Value> {
        self.call(reqwest::Method::GET, "/wolfmail/booking-links", &[], None).await
    }

    pub async fn business_mailboxes(&self) -> Result<Value> {
        self.call(reqwest::Method::GET, "/wolfmail/business-mailboxes", &[], None).await
    }

    /// Which of these addresses are people in IW → `{ matches: { email: { kind, id, name, url } } }`.
    pub async fn people(&self, emails: &[String]) -> Result<Value> {
        self.call(reqwest::Method::POST, "/wolfmail/people", &[], Some(json!({ "emails": emails }))).await
    }

    /// Turn a question into a WolfMail search query → `{ query, explanation, creditsUsed }`. Only the question is sent.
    pub async fn search_query(&self, body: Value) -> Result<Value> {
        self.call(reqwest::Method::POST, "/wolfmail/search-query", &[], Some(body)).await
    }

    pub async fn contact_update(&self, body: Value) -> Result<Value> {
        self.call(reqwest::Method::PATCH, "/wolfmail/contact", &[], Some(body)).await
    }

    pub async fn contact_create(&self, body: Value) -> Result<Value> {
        self.call(reqwest::Method::POST, "/wolfmail/contacts", &[], Some(body)).await
    }

    pub async fn chat_send(&self, body: Value) -> Result<Value> {
        self.call(reqwest::Method::POST, "/wolfmail/chat", &[], Some(body)).await
    }

    pub async fn chat_turn(&self, turn: &str) -> Result<Value> {
        // Source: IntelligentWolf's WolfMail routes: wolfmail_turn_load() — 'wmt-' + 20 hex.
        if turn.len() != 24 || !turn.starts_with("wmt-") || !turn[4..].chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ApiError::invalid("turn", "Not a chat message id."));
        }
        self.call(reqwest::Method::GET, &format!("/wolfmail/chat/{turn}"), &[], None).await
    }

    pub async fn chat_history(&self) -> Result<Value> {
        self.call(reqwest::Method::GET, "/wolfmail/chat", &[], None).await
    }

    pub async fn disconnect(&self) -> Result<()> {
        if self.device()?.is_some() {
            // Tell the server first; whatever it says, the local token goes.
            let _ = self.call(reqwest::Method::DELETE, "/wolfmail/device", &[], None).await;
        }
        self.secrets.delete(IW_DEVICE_KEY)
    }
}

/// The assistant's name: 1–24 letters, spaces, hyphens or apostrophes, starting with a letter.
/// Source: IntelligentWolf's WolfMail routes, wolfmail_assistant_name() — anything else is "Alex" there.
pub fn valid_assistant_name(name: &str) -> bool {
    let n = name.trim();
    let count = n.chars().count();
    (1..=24).contains(&count)
        && n.chars().next().is_some_and(char::is_alphabetic)
        && n.chars().all(|c| c.is_alphabetic() || c == ' ' || c == '\'' || c == '-')
}

/// Truncate to N characters (the AI route takes at most 20 000 — docs/IW-INTEGRATION.md).
pub fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn pairing_and_device_routes_against_mock() {
        crate::tls::install_crypto_provider();
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(method("POST"))
            .and(path("/wolfmail/pair/start"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "pairId": "p1", "pollSecret": "s1", "code": "ABCD-EFGH",
                "verifyUrl": "https://intelligentwolf.com/app/wolfmail/pair?code=ABCD-EFGH", "expiresAt": 1793095800000i64
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/wolfmail/pair/poll"))
            .and(body_json(json!({ "pairId": "p1", "pollSecret": "s1" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "state": "done", "deviceToken": "wmd_abc", "user": { "name": "Paul", "email": "paul@example.test" }, "org": { "id": 7, "name": "Wolf" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/wolfmail/person"))
            .and(query_param("email", "ann@example.com"))
            .and(header("authorization", "Bearer wmd_abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "kind": "lead", "name": "Ann", "email": "ann@example.com", "url": "https://x", "timeline": [] })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/wolfmail/person"))
            .and(query_param("email", "nobody@example.com"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "kind": "none" })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/wolfmail/ai"))
            .respond_with(ResponseTemplate::new(402).set_body_json(json!({ "error": "no_credits" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/wolfmail/booking-links"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let secrets = MemoryStore::default();
        let c = IwClient { http: &http, secrets: &secrets };
        let start = c.pair_start(&base, "test", "0.1.0").await.unwrap();
        assert_eq!(start.code, "ABCD-EFGH");
        assert_eq!(c.pair_poll(&base, &start).await.unwrap(), "done");
        assert!(c.status("x").unwrap().connected);
        let p = c.person("ann@example.com").await.unwrap();
        assert_eq!(p["kind"], "lead");
        assert!(c.person("nobody@example.com").await.unwrap().is_null());
        let e = c.ai(json!({})).await.err().unwrap();
        assert!(e.message.contains("credits"));
        // A 401 forgets the device.
        let e = c.booking_links().await.err().unwrap();
        assert_eq!(e.kind, ErrorKind::Auth);
        assert!(!c.status("x").unwrap().connected);
    }

    #[tokio::test]
    async fn people_contacts_and_chat_routes_against_mock() {
        crate::tls::install_crypto_provider();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/wolfmail/people"))
            .and(header("authorization", "Bearer wmd_abc"))
            .and(body_json(json!({ "emails": ["ann@example.com"] })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "matches": { "ann@example.com": { "kind": "lead", "id": "12", "name": "Ann", "url": "https://x" } } })))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/wolfmail/contact"))
            .and(body_json(json!({ "kind": "lead", "id": "12", "fields": { "phone": "0161" }, "note": "Wants a quote" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "kind": "lead", "name": "Ann", "leadId": "12", "url": "https://x", "timeline": [] })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/wolfmail/chat"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "turn": "wmt-0123456789abcdef0123", "status": "busy" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/wolfmail/chat/wmt-0123456789abcdef0123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "turn": "wmt-0123456789abcdef0123", "status": "done", "reply": "Hello" })))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let secrets = MemoryStore::default();
        let dev = IwDevice {
            device_token: "wmd_abc".into(),
            user: IwUser { name: "Paul".into(), email: "p@x".into() },
            org: IwOrg { id: json!("org-a"), name: "Wolf".into() },
            base_url: server.uri(),
        };
        secrets.set(IW_DEVICE_KEY, &serde_json::to_string(&dev).unwrap()).unwrap();
        let c = IwClient { http: &http, secrets: &secrets };
        let m = c.people(&["ann@example.com".into()]).await.unwrap();
        assert_eq!(m["matches"]["ann@example.com"]["kind"], "lead");
        let p = c.contact_update(json!({ "kind": "lead", "id": "12", "fields": { "phone": "0161" }, "note": "Wants a quote" })).await.unwrap();
        assert_eq!(p["leadId"], "12");
        let t = c.chat_send(json!({ "message": "hi" })).await.unwrap();
        let turn = t["turn"].as_str().unwrap();
        assert_eq!(c.chat_turn(turn).await.unwrap()["reply"], "Hello");
        // A turn id is only ever wmt-<hex>: nothing else reaches the URL path.
        assert!(c.chat_turn("../device").await.is_err());
        assert!(c.chat_turn("wmt-zz").await.is_err());
    }

    #[test]
    fn assistant_names() {
        assert!(valid_assistant_name("Alex"));
        assert!(valid_assistant_name("D'Arcy-Jo"));
        assert!(valid_assistant_name("Zoë"));
        assert!(!valid_assistant_name(""));
        assert!(!valid_assistant_name("  "));
        assert!(!valid_assistant_name("-Alex"));
        assert!(!valid_assistant_name("Alex<b>"));
        assert!(!valid_assistant_name(&"a".repeat(25)));
    }

    #[test]
    fn base_url_must_be_https() {
        assert!(validate_base("http://intelligentwolf.com/api").is_err());
        assert!(validate_base("https://intelligentwolf.com/api/").is_ok());
        assert!(validate_base("http://127.0.0.1:9999").is_ok());
    }
}
