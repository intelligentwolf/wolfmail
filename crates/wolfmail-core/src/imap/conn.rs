//! One authenticated IMAP connection: TLS / STARTTLS / plain (localhost only), LOGIN or XOAUTH2, capability
//! discovery, ENABLE QRESYNC, and a raw command runner that hands back every untagged response so QRESYNC
//! VANISHED and Gmail X-GM-* attributes are never lost (async-imap's unsolicited channel is bounded at 100 and
//! drops when full — Source: async-imap 0.12 client.rs:355 `bounded(100)` + parse.rs `try_send(..).ok()`).
use crate::error::{ApiError, ErrorKind, Result};
use crate::model::{Security, ServerConfig};
use crate::tls;
use async_imap::imap_proto::{self, Response, Status};
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

pub enum ImapStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl std::fmt::Debug for ImapStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ImapStream::Plain(_) => "ImapStream::Plain",
            ImapStream::Tls(_) => "ImapStream::Tls",
        })
    }
}

impl AsyncRead for ImapStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ImapStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            ImapStream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for ImapStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            ImapStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            ImapStream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ImapStream::Plain(s) => Pin::new(s).poll_flush(cx),
            ImapStream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            ImapStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            ImapStream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

pub type Session = async_imap::Session<ImapStream>;

/// An owned IMAP response. async-imap's `ResponseData` cannot be named outside that crate (it is re-exported
/// `pub(crate)`), so each one is re-parsed from its bytes into an owned `imap_proto::Response<'static>`.
pub struct Resp(Response<'static>);

impl Resp {
    pub fn parsed(&self) -> &Response<'static> {
        &self.0
    }
}

/// Re-parse an async-imap response's own bytes into an owned Response.
pub(crate) fn to_owned_resp(bytes: &[u8]) -> Result<Resp> {
    match Response::parse(bytes) {
        Ok((_, r)) => Ok(Resp(r.into_owned())),
        Err(_) => Err(ApiError::server("Unexpected reply from the mail server.")),
    }
}

#[derive(Clone)]
pub enum Credential {
    Password { user: String, pass: String },
    OAuth { user: String, token: String },
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the secret.
        match self {
            Credential::Password { user, .. } => write!(f, "Password({user})"),
            Credential::OAuth { user, .. } => write!(f, "OAuth({user})"),
        }
    }
}

/// Source: Microsoft Learn "Authenticate an IMAP, POP or SMTP connection using OAuth" — SASL XOAUTH2 is
/// base64("user=" + user + "^Aauth=Bearer " + token + "^A^A"); async-imap base64-encodes what `process` returns.
pub fn xoauth2_payload(user: &str, token: &str) -> String {
    format!("user={user}\x01auth=Bearer {token}\x01\x01")
}

struct XOAuth2(String);
impl async_imap::Authenticator for XOAuth2 {
    type Response = String;
    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        // A second challenge means the server rejected the token (it sends a JSON error); answering with an empty
        // response makes it finish with NO, which we map to an auth error.
        std::mem::take(&mut self.0)
    }
}

pub fn is_localhost(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

pub async fn tcp_connect(host: &str, port: u16) -> Result<TcpStream> {
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port))).await {
        Err(_) => Err(ApiError::network(format!("Timed out connecting to {host}:{port}."))),
        Ok(Err(e)) => Err(ApiError::network(format!("Could not connect to {host}:{port}: {e}"))),
        Ok(Ok(s)) => {
            let _ = s.set_nodelay(true);
            Ok(s)
        }
    }
}

pub async fn tls_wrap(host: &str, tcp: TcpStream) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let cfg = tls::client_config()?;
    let connector = tokio_rustls::TlsConnector::from(Arc::clone(&cfg));
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| ApiError::invalid("host", format!("“{host}” is not a valid server name.")))?;
    match tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(name, tcp)).await {
        Err(_) => Err(ApiError::network(format!("Timed out during the secure handshake with {host}."))),
        Ok(Err(e)) => Err(tls::tls_error(host, e)),
        Ok(Ok(s)) => Ok(s),
    }
}

pub fn map_imap_err(e: async_imap::error::Error) -> ApiError {
    use async_imap::error::Error as E;
    match e {
        E::Io(io) => ApiError::from(io),
        E::ConnectionLost => ApiError::network("The mail server closed the connection."),
        E::No(m) => ApiError::server(format!("The mail server refused: {}", m.trim())),
        E::Bad(m) => ApiError::server(format!("The mail server did not understand a command: {}", m.trim())),
        E::Parse(p) => ApiError::server(format!("Unexpected reply from the mail server: {p}")),
        E::Validate(v) => ApiError::invalid("folder", format!("Invalid characters in a folder or argument: {v}")),
        other => ApiError::server(format!("Mail server error: {other}")),
    }
}

pub struct Conn {
    pub session: Session,
    pub caps: HashSet<String>,
    pub selected: Option<String>,
    pub qresync: bool,
    pub host: String,
}

impl Conn {
    pub fn has(&self, cap: &str) -> bool {
        self.caps.contains(&cap.to_ascii_uppercase())
    }
    pub fn condstore(&self) -> bool {
        self.qresync || self.has("CONDSTORE")
    }
    pub fn is_gmail(&self) -> bool {
        self.has("X-GM-EXT-1")
    }
}

pub async fn connect(cfg: &ServerConfig, cred: &Credential) -> Result<Conn> {
    let host = cfg.host.trim();
    if host.is_empty() {
        return Err(ApiError::invalid("imap.host", "Enter the incoming (IMAP) server name."));
    }
    if cfg.security == Security::None && !is_localhost(host) {
        return Err(ApiError::invalid(
            "imap.security",
            "Unencrypted connections are only allowed to this computer. Choose SSL/TLS or STARTTLS.",
        ));
    }
    let tcp = tcp_connect(host, cfg.port).await?;
    let mut client = match cfg.security {
        Security::Tls => async_imap::Client::new(ImapStream::Tls(Box::new(tls_wrap(host, tcp).await?))),
        Security::None | Security::Starttls => async_imap::Client::new(ImapStream::Plain(tcp)),
    };
    // Greeting.
    match tokio::time::timeout(CONNECT_TIMEOUT, client.read_response()).await {
        Err(_) => return Err(ApiError::network(format!("{host} did not send an IMAP greeting."))),
        Ok(Err(e)) => return Err(ApiError::from(e)),
        Ok(Ok(None)) => return Err(ApiError::network(format!("{host} closed the connection immediately."))),
        Ok(Ok(Some(g))) => {
            if let Response::Data { status: Status::Bye, .. } = g.parsed() {
                return Err(ApiError::server(format!("{host} refused the connection (BYE).")));
            }
        }
    }
    if cfg.security == Security::Starttls {
        client.run_command_and_check_ok("STARTTLS", None).await.map_err(|e| {
            ApiError::new(ErrorKind::Tls, format!("{host} does not support STARTTLS ({}).", map_imap_err(e).message))
        })?;
        let ImapStream::Plain(tcp) = client.into_inner() else {
            return Err(ApiError::internal("STARTTLS on an already-secure stream."));
        };
        client = async_imap::Client::new(ImapStream::Tls(Box::new(tls_wrap(host, tcp).await?)));
    }
    let login = async {
        match cred {
            Credential::Password { user, pass } => client.login(user, pass).await,
            Credential::OAuth { user, token } => client.authenticate("XOAUTH2", XOAuth2(xoauth2_payload(user, token))).await,
        }
    };
    let mut session = match tokio::time::timeout(COMMAND_TIMEOUT, login).await {
        Err(_) => return Err(ApiError::network(format!("{host} did not answer the sign-in."))),
        Ok(Err((e, _))) => {
            let base = map_imap_err(e);
            if base.kind != ErrorKind::Server {
                return Err(base);
            }
            // A refused login is not always a wrong password. RFC 5530 response codes say which: UNAVAILABLE,
            // LIMIT, INUSE and SERVERBUG are temporary (retry later, keep the password); AUTHENTICATIONFAILED and
            // friends mean the credentials are wrong. Servers that send no code but say "too many"/"try again"
            // are treated as temporary too. Seen 28 Oct 2026: two StackMail accounts that had been syncing were
            // marked "check your password" after a burst of reconnects, and auth errors stop retrying by design.
            let said = base.message.to_ascii_uppercase();
            let temporary = ["[UNAVAILABLE]", "[LIMIT]", "[INUSE]", "[SERVERBUG]", "TOO MANY", "TRY AGAIN", "TEMPORAR", "RATE LIMIT"]
                .iter()
                .any(|k| said.contains(k));
            let server_words = base.message.trim_start_matches("The mail server refused: ").to_string();
            return Err(if temporary {
                ApiError::network(format!("{host} is refusing sign-ins for the moment ({server_words}). WolfMail will try again."))
            } else {
                ApiError::auth(format!(
                    "Sign-in to {host} failed{}. Check the user name and password{}.",
                    if server_words.is_empty() { String::new() } else { format!(" ({server_words})") },
                    match cred {
                        Credential::OAuth { .. } => " (or reconnect the account)",
                        _ => "",
                    }
                ))
            });
        }
        Ok(Ok(s)) => s,
    };
    let caps: HashSet<String> = session
        .capabilities()
        .await
        .map_err(map_imap_err)?
        .iter()
        .map(|c| match c {
            async_imap::types::Capability::Imap4rev1 => "IMAP4REV1".to_string(),
            async_imap::types::Capability::Auth(a) => format!("AUTH={}", a.to_ascii_uppercase()),
            async_imap::types::Capability::Atom(a) => a.to_ascii_uppercase(),
        })
        .collect();
    let mut conn = Conn { session, caps, selected: None, qresync: false, host: host.to_string() };
    // Source: RFC 7162 §3.2.3 — QRESYNC must be ENABLEd (RFC 5161) before SELECT … (QRESYNC …) is allowed.
    // Source: RFC 5161 §3.2 — the server answers "* ENABLED QRESYNC" for what it switched on; imap-proto
    // parses that as Response::Capabilities (imap-proto rfc5161.rs resp_enabled).
    if conn.has("QRESYNC") && conn.has("ENABLE") {
        if let Ok(resps) = conn.raw("ENABLE QRESYNC").await {
            conn.qresync = resps.iter().any(|r| match r.parsed() {
                Response::Capabilities(c) => c.iter().any(|x| {
                    matches!(x, imap_proto::Capability::Atom(a) if a.eq_ignore_ascii_case("QRESYNC"))
                }),
                _ => false,
            });
        }
    }
    Ok(conn)
}

impl Conn {
    /// Run one tagged command; returns every untagged response that arrived before the tagged OK.
    pub async fn raw(&mut self, cmd: &str) -> Result<Vec<Resp>> {
        let fut = async {
            let id = self.session.run_command(cmd).await.map_err(map_imap_err)?;
            let mut out = Vec::new();
            loop {
                let Some(rd) = self.session.read_response().await? else {
                    return Err(ApiError::network("The mail server closed the connection."));
                };
                let resp = to_owned_resp(&rd.into_owner())?;
                let done = match resp.parsed() {
                    Response::Done { tag, status, outcome } if *tag == id => Some((status, outcome.information.as_deref().unwrap_or("").to_string())),
                    Response::Data { status: Status::Bye, outcome } => {
                        return Err(ApiError::network(format!(
                            "The mail server ended the session: {}",
                            outcome.information.as_deref().unwrap_or("")
                        )))
                    }
                    _ => None,
                };
                match done {
                    Some((Status::Ok, _)) => {
                        out.push(resp);
                        return Ok(out);
                    }
                    Some((Status::No, info)) => return Err(ApiError::server(format!("The mail server refused: {info}"))),
                    Some((_, info)) => return Err(ApiError::server(format!("The mail server rejected a command: {info}"))),
                    None => out.push(resp),
                }
            }
        };
        match tokio::time::timeout(COMMAND_TIMEOUT, fut).await {
            Err(_) => Err(ApiError::network(format!("{} stopped responding.", self.host))),
            Ok(r) => r,
        }
    }

    pub async fn logout(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(5), self.session.logout()).await;
    }
}

/// Quote a string for an IMAP command (RFC 3501 §4.3 quoted string).
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xoauth2_format_matches_microsoft_doc() {
        use base64::Engine as _;
        // Source: Microsoft Learn example — test@contoso.onmicrosoft.com + EwBAAl3BAAUFFpUAo7J3Ve0bjLBWZWCclRC3EoAA
        let enc = base64::engine::general_purpose::STANDARD
            .encode(xoauth2_payload("test@contoso.onmicrosoft.com", "EwBAAl3BAAUFFpUAo7J3Ve0bjLBWZWCclRC3EoAA"));
        assert_eq!(
            enc,
            "dXNlcj10ZXN0QGNvbnRvc28ub25taWNyb3NvZnQuY29tAWF1dGg9QmVhcmVyIEV3QkFBbDNCQUFVRkZwVUFvN0ozVmUwYmpMQldaV0NjbFJDM0VvQUEBAQ=="
        );
    }
    #[test]
    fn quoting() {
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
