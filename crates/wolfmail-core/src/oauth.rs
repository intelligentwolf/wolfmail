//! OAuth 2.0 for Google and Microsoft mail: authorization code + PKCE (RFC 7636) through the system browser with a
//! loopback redirect (RFC 8252 §7.3), refresh tokens kept in the OS keychain.
//!
//! Sources (fetched 2026-10-28):
//! - Google "OAuth 2.0 for Mobile & Desktop Apps": auth `https://accounts.google.com/o/oauth2/v2/auth`, token
//!   `https://oauth2.googleapis.com/token`, redirect `http://127.0.0.1:port`, `code_challenge_method=S256`;
//!   refresh = POST client_id + refresh_token + grant_type=refresh_token. Desktop clients are public clients;
//!   Google still issues a (non-confidential) client secret for "Desktop app" credentials, sent when configured.
//! - Google "OAuth 2.0 for Mobile & Desktop Apps" (re-read 2026-10-28): create a "Desktop app" client; loopback
//!   redirect `http://127.0.0.1:port`; "Desktop apps are public clients and cannot keep secrets confidential" —
//!   client_secret is optional on the token requests; always use PKCE.
//! - Scopes are the ones IntelligentWolf's Google app is already approved for (../intelligentwolf/php-api/lib/
//!   gmail.php:28-50 GMAIL_SCOPES): gmail.modify, gmail.send, gmail.settings.basic, openid, email. NOT
//!   https://mail.google.com/ (IMAP/SMTP), which that app is not approved for — mail goes through the Gmail API.
//! - Microsoft Learn "OAuth 2.0 authorization code flow" (re-read 2026-10-28): `https://login.microsoftonline.com/
//!   {tenant}/oauth2/v2.0/authorize|token`, tenant `common` (as IW: php-api/lib/microsoft.php:11,125); public
//!   clients "must not use secrets"; PKCE recommended; desktop apps using the system browser use redirect
//!   `http://localhost`; refresh tokens only with `offline_access`.
//! - Scopes = IW's Entra app (php-api/lib/microsoft.php:20-29 MS_SCOPES) minus Mail.Read (Mail.ReadWrite covers it):
//!   openid profile email offline_access Mail.ReadWrite Mail.Send User.Read — Microsoft Graph, not IMAP/SMTP.
//! - Microsoft Learn "Redirect URI best practices": for localhost redirect URIs the port is ignored when matching,
//!   so `http://localhost` registered once matches `http://localhost:<any port>`; IPv6 [::1] is not supported.
use crate::error::{ApiError, ErrorKind, Result};
use crate::secrets::{oauth_key, SecretStore};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OAuthProvider {
    Google,
    Microsoft,
}

struct ProviderCfg {
    auth_url: &'static str,
    token_url: &'static str,
    scopes: &'static str,
    client_id: Option<&'static str>,
    client_secret: Option<&'static str>,
    env_name: &'static str,
}

fn cfg(p: OAuthProvider) -> ProviderCfg {
    match p {
        OAuthProvider::Google => ProviderCfg {
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            scopes: "https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.send https://www.googleapis.com/auth/gmail.settings.basic openid email",
            client_id: option_env!("WOLFMAIL_GOOGLE_CLIENT_ID"),
            client_secret: option_env!("WOLFMAIL_GOOGLE_CLIENT_SECRET"),
            env_name: "WOLFMAIL_GOOGLE_CLIENT_ID",
        },
        OAuthProvider::Microsoft => ProviderCfg {
            auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize",
            token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
            scopes: "openid profile email offline_access Mail.ReadWrite Mail.Send User.Read",
            client_id: option_env!("WOLFMAIL_MS_CLIENT_ID"),
            client_secret: None,
            env_name: "WOLFMAIL_MS_CLIENT_ID",
        },
    }
}

pub fn is_configured(p: OAuthProvider) -> bool {
    cfg(p).client_id.is_some_and(|s| !s.is_empty())
}

fn not_configured(p: OAuthProvider) -> ApiError {
    let c = cfg(p);
    ApiError::new(
        ErrorKind::OauthConfig,
        format!(
            "{} sign-in is not set up in this build of WolfMail (missing {}). Use an app password with IMAP for now, or install an official WolfMail release.",
            match p {
                OAuthProvider::Google => "Google",
                OAuthProvider::Microsoft => "Microsoft",
            },
            c.env_name
        ),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredToken {
    provider: OAuthProvider,
    refresh_token: String,
    access_token: String,
    expires_at: i64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn pkce_pair() -> (String, String) {
    let mut buf = [0u8; 32];
    rand::fill(&mut buf);
    let verifier = b64url(&buf);
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn random_state() -> String {
    let mut buf = [0u8; 16];
    rand::fill(&mut buf);
    b64url(&buf)
}

/// Email claim of an id_token received directly from the provider's token endpoint over TLS (signature checking
/// is not needed for a token obtained that way — Google "OpenID Connect", "Obtain user information from the ID
/// token"; Microsoft id_token `email` / `preferred_username`).
fn id_token_email(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("email").or_else(|| v.get("preferred_username")).and_then(|e| e.as_str()).map(|s| s.to_ascii_lowercase())
}

/// Parse "GET /?code=..&state=.. HTTP/1.1" from the loopback request.
fn parse_redirect(req: &str) -> Option<Vec<(String, String)>> {
    let line = req.lines().next()?;
    let path = line.split_whitespace().nth(1)?;
    let url = url::Url::parse(&format!("http://127.0.0.1{path}")).ok()?;
    Some(url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect())
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>WolfMail</title><body style=\"font-family:system-ui;background:#2e2f37;color:#eceff1;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\"><h1 style=\"font-weight:500\">You're signed in</h1><p>You can close this tab and go back to WolfMail.</p></div>";
const FAIL_PAGE: &str = "<!doctype html><meta charset=utf-8><title>WolfMail</title><body style=\"font-family:system-ui;background:#2e2f37;color:#eceff1;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\"><h1 style=\"font-weight:500\">Sign-in did not finish</h1><p>Go back to WolfMail to see what happened.</p></div>";

/// The authorization request (system browser). Public client: PKCE S256, no secret anywhere in it.
pub fn auth_url(provider: OAuthProvider, client_id: &str, redirect: &str, state: &str, challenge: &str, email: &str) -> Result<url::Url> {
    let c = cfg(provider);
    let mut auth = url::Url::parse(c.auth_url).map_err(|e| ApiError::internal(e.to_string()))?;
    {
        let mut q = auth.query_pairs_mut();
        q.append_pair("client_id", client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", redirect)
            .append_pair("scope", c.scopes)
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("login_hint", email);
        if provider == OAuthProvider::Google {
            // Source: Google OAuth docs — access_type=offline returns a refresh token; prompt=consent makes sure one
            // is issued again on re-authorisation.
            q.append_pair("access_type", "offline").append_pair("prompt", "consent");
        } else {
            q.append_pair("response_mode", "query").append_pair("prompt", "select_account");
        }
    }
    Ok(auth)
}

pub async fn authorize(
    http: &reqwest::Client,
    secrets: &dyn SecretStore,
    provider: OAuthProvider,
    email: &str,
    open_browser: &(dyn Fn(&str) -> Result<()> + Send + Sync),
) -> Result<String> {
    let c = cfg(provider);
    let client_id = c.client_id.filter(|s| !s.is_empty()).ok_or_else(|| not_configured(provider))?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    // Google: http://127.0.0.1:port. Microsoft: the registered "http://localhost" (port ignored for localhost) —
    // the browser may resolve localhost to ::1 first, so the same port is also opened on [::1] when possible.
    let redirect = match provider {
        OAuthProvider::Google => format!("http://127.0.0.1:{port}"),
        OAuthProvider::Microsoft => format!("http://localhost:{port}"),
    };
    let listener6 = if provider == OAuthProvider::Microsoft { TcpListener::bind(("::1", port)).await.ok() } else { None };
    let (verifier, challenge) = pkce_pair();
    let state = random_state();
    let auth = auth_url(provider, client_id, &redirect, &state, &challenge, email)?;
    open_browser(auth.as_str())?;

    let wait = async {
        loop {
            let (mut sock, _) = match &listener6 {
                Some(l6) => tokio::select! {
                    r = listener.accept() => r?,
                    r = l6.accept() => r?,
                },
                None => listener.accept().await?,
            };
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await?;
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let Some(pairs) = parse_redirect(&req) else {
                let _ = sock.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                continue;
            };
            let get = |k: &str| pairs.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
            // Browsers also ask for /favicon.ico — ignore anything without our parameters.
            if get("code").is_none() && get("error").is_none() {
                let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                continue;
            }
            let ok = get("state").as_deref() == Some(state.as_str()) && get("code").is_some();
            let page = if ok { DONE_PAGE } else { FAIL_PAGE };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{}",
                page.len(),
                page
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
            if let Some(err) = get("error") {
                let desc = get("error_description").unwrap_or_default();
                return Err(if err == "access_denied" {
                    ApiError::new(ErrorKind::Cancelled, "Sign-in was cancelled.")
                } else {
                    ApiError::auth(format!("The provider refused sign-in: {err} {desc}"))
                });
            }
            if get("state").as_deref() != Some(state.as_str()) {
                return Err(ApiError::auth("The sign-in response did not match this request. Try again."));
            }
            return Ok(get("code").unwrap_or_default());
        }
    };
    let code = match tokio::time::timeout(Duration::from_secs(300), wait).await {
        Err(_) => return Err(ApiError::new(ErrorKind::Cancelled, "Sign-in timed out after 5 minutes.")),
        Ok(r) => r?,
    };

    let mut form = vec![
        ("client_id", client_id.to_string()),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code".to_string()),
        ("redirect_uri", redirect),
    ];
    if let Some(s) = c.client_secret.filter(|s| !s.is_empty()) {
        form.push(("client_secret", s.to_string()));
    }
    let tok = token_request(http, c.token_url, &form).await?;
    let refresh = tok
        .refresh_token
        .clone()
        .ok_or_else(|| ApiError::auth("The provider did not return a refresh token. Remove WolfMail's access in your account settings and try again."))?;
    let signed_in = tok.id_token.as_deref().and_then(id_token_email);
    if let Some(e) = &signed_in {
        if !e.eq_ignore_ascii_case(email) {
            return Err(ApiError::invalid("email", format!("You signed in as {e}, but this account is {email}. Sign in with {email}.")));
        }
    }
    let stored = StoredToken {
        provider,
        refresh_token: refresh,
        access_token: tok.access_token,
        expires_at: crate::db::now_ms() + tok.expires_in.unwrap_or(3600) * 1000,
    };
    secrets.set(&oauth_key(email), &serde_json::to_string(&stored)?)?;
    Ok(signed_in.unwrap_or_else(|| email.to_ascii_lowercase()))
}

async fn token_request(http: &reqwest::Client, url: &str, form: &[(&str, String)]) -> Result<TokenResponse> {
    let resp = http.post(url).form(form).send().await?;
    let status = resp.status();
    let body = resp.bytes().await?;
    if status.is_success() {
        return serde_json::from_slice(&body).map_err(|e| ApiError::server(format!("Unexpected token response: {e}")));
    }
    match serde_json::from_slice::<TokenError>(&body) {
        Ok(e) if e.error == "invalid_grant" => Err(ApiError::auth("Your sign-in has expired or was revoked. Reconnect the account.")),
        Ok(e) => Err(ApiError::auth(format!("Sign-in failed: {} {}", e.error, e.error_description.unwrap_or_default()))),
        Err(_) => Err(ApiError::server(format!("Sign-in failed (HTTP {status}).")) ),
    }
}

/// Store tokens obtained elsewhere (re-sign-in flows, tests) in the same shape `authorize` writes.
pub fn save_token(secrets: &dyn SecretStore, email: &str, provider: OAuthProvider, refresh_token: &str, access_token: &str, expires_at: i64) -> Result<()> {
    let st = StoredToken { provider, refresh_token: refresh_token.into(), access_token: access_token.into(), expires_at };
    secrets.set(&oauth_key(email), &serde_json::to_string(&st)?)
}

/// A valid access token for `email`, refreshing it when it expires within a minute.
pub async fn access_token(http: &reqwest::Client, secrets: &dyn SecretStore, email: &str) -> Result<String> {
    let key = oauth_key(email);
    let raw = secrets.get(&key)?.ok_or_else(|| ApiError::auth("This account needs to be signed in again."))?;
    let mut st: StoredToken = serde_json::from_str(&raw)?;
    if st.expires_at - 60_000 > crate::db::now_ms() && !st.access_token.is_empty() {
        return Ok(st.access_token);
    }
    let c = cfg(st.provider);
    let client_id = c.client_id.filter(|s| !s.is_empty()).ok_or_else(|| not_configured(st.provider))?;
    let mut form = vec![
        ("client_id", client_id.to_string()),
        ("refresh_token", st.refresh_token.clone()),
        ("grant_type", "refresh_token".to_string()),
    ];
    if st.provider == OAuthProvider::Microsoft {
        form.push(("scope", c.scopes.to_string()));
    }
    if let Some(s) = c.client_secret.filter(|s| !s.is_empty()) {
        form.push(("client_secret", s.to_string()));
    }
    let tok = token_request(http, c.token_url, &form).await?;
    st.access_token = tok.access_token;
    st.expires_at = crate::db::now_ms() + tok.expires_in.unwrap_or(3600) * 1000;
    // Source: RFC 6749 §6 / Microsoft doc — a new refresh token replaces the old one.
    if let Some(r) = tok.refresh_token {
        st.refresh_token = r;
    }
    secrets.set(&key, &serde_json::to_string(&st)?)?;
    Ok(st.access_token)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pkce_is_s256_of_verifier() {
        let (v, c) = pkce_pair();
        assert_eq!(v.len(), 43);
        assert_eq!(c, b64url(&Sha256::digest(v.as_bytes())));
    }
    #[test]
    fn auth_urls_request_the_api_scopes_only() {
        let g = auth_url(OAuthProvider::Google, "cid", "http://127.0.0.1:5000", "st", "ch", "me@gmail.com").unwrap();
        let q: std::collections::HashMap<String, String> = g.query_pairs().into_owned().collect();
        assert_eq!(g.host_str(), Some("accounts.google.com"));
        assert_eq!(q["scope"], "https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.send https://www.googleapis.com/auth/gmail.settings.basic openid email");
        assert!(!q["scope"].contains("mail.google.com"));
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:5000");
        let m = auth_url(OAuthProvider::Microsoft, "cid", "http://localhost:5000", "st", "ch", "me@contoso.com").unwrap();
        let q: std::collections::HashMap<String, String> = m.query_pairs().into_owned().collect();
        assert_eq!(m.path(), "/common/oauth2/v2.0/authorize");
        assert_eq!(q["scope"], "openid profile email offline_access Mail.ReadWrite Mail.Send User.Read");
        assert!(!q["scope"].contains("IMAP") && !q["scope"].contains("SMTP"));
        assert!(!m.as_str().contains("client_secret"));
        // A public client never carries a Microsoft secret.
        assert!(cfg(OAuthProvider::Microsoft).client_secret.is_none());
    }

    #[test]
    fn redirect_parsing() {
        let p = parse_redirect("GET /?state=abc&code=4%2F0Ax HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
        assert!(p.contains(&("code".into(), "4/0Ax".into())));
        assert!(p.contains(&("state".into(), "abc".into())));
    }
    #[test]
    fn id_token_claim() {
        let payload = b64url(br#"{"email":"Paul@Example.com"}"#);
        assert_eq!(id_token_email(&format!("x.{payload}.y")).as_deref(), Some("paul@example.com"));
    }
    #[test]
    fn unconfigured_build_says_so() {
        if !is_configured(OAuthProvider::Google) {
            assert_eq!(not_configured(OAuthProvider::Google).kind, ErrorKind::OauthConfig);
        }
    }
}
