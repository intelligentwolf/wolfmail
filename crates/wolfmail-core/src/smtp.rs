//! Building outgoing MIME (mail-builder) and submitting it (lettre, async, rustls).
// Source: RFC 5322 §3.6.4 (Message-ID / In-Reply-To / References), RFC 6409 (submission on 587 + STARTTLS,
// 465 implicit TLS per RFC 8314 §3.3), Microsoft Learn XOAUTH2 doc for the SASL format lettre implements.
use crate::error::{ApiError, ErrorKind, Result};
use crate::imap::conn::is_localhost;
use crate::mime::html_to_text;
use crate::model::{Address, DraftInput, Identity, Security, ServerConfig};
use base64::Engine as _;
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use mail_builder::MessageBuilder;
use regex::Regex;
use std::sync::OnceLock;
use std::time::Duration;

pub struct ResolvedAttachment {
    pub filename: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

pub struct BuiltMessage {
    /// What is handed to the SMTP server (no Bcc header).
    pub wire: Vec<u8>,
    /// What is stored in Sent / Drafts (keeps Bcc so the sender can see who was blind-copied).
    pub copy: Vec<u8>,
    pub message_id: String,
    pub envelope_from: String,
    pub recipients: Vec<String>,
}

fn domain_of(email: &str) -> &str {
    email.rsplit_once('@').map(|(_, d)| d).filter(|d| !d.is_empty()).unwrap_or("wolfmail.invalid")
}

pub fn new_message_id(identity_email: &str) -> String {
    format!("{}@{}", uuid::Uuid::new_v4().simple(), domain_of(identity_email))
}

fn to_builder_addrs(list: &[Address]) -> Vec<(String, String)> {
    list.iter().map(|a| (a.name.clone().unwrap_or_default(), a.email.clone())).collect()
}

/// Pasted images arrive from the composer as data: URIs; recipients' clients often block those, so they become
/// cid: inline parts (RFC 2392).
fn extract_inline_images(html: &str, domain: &str) -> (String, Vec<(String, String, Vec<u8>)>) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"src\s*=\s*["']data:(image/[a-zA-Z0-9.+-]+);base64,([A-Za-z0-9+/=\s]+)["']"#).unwrap());
    let mut parts = Vec::new();
    let out = re.replace_all(html, |c: &regex::Captures| {
        let mime = c[1].to_string();
        let data: String = c[2].chars().filter(|ch| !ch.is_whitespace()).collect();
        match base64::engine::general_purpose::STANDARD.decode(data) {
            Ok(bytes) => {
                let cid = format!("{}@{}", uuid::Uuid::new_v4().simple(), domain);
                let s = format!("src=\"cid:{cid}\"");
                parts.push((cid, mime, bytes));
                s
            }
            Err(_) => c[0].to_string(),
        }
    });
    (out.into_owned(), parts)
}

pub fn build_message(
    draft: &DraftInput,
    identity: &Identity,
    message_id: &str,
    attachments: &[ResolvedAttachment],
    date_ms: i64,
    for_send: bool,
) -> Result<BuiltMessage> {
    // Drafts may be half-written; only a message being sent must have valid recipients.
    if for_send {
        if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
            return Err(ApiError::invalid("to", "Add at least one recipient."));
        }
        for a in draft.to.iter().chain(&draft.cc).chain(&draft.bcc) {
            if a.email.parse::<lettre::Address>().is_err() {
                return Err(ApiError::invalid("to", format!("“{}” is not a valid email address.", a.email)));
            }
        }
    }
    let (html, inline) = extract_inline_images(&draft.html, domain_of(&identity.email));
    let text = if draft.text.trim().is_empty() { html_to_text(&html) } else { draft.text.clone() };

    let build = |with_bcc: bool| -> Result<Vec<u8>> {
        let mut b = MessageBuilder::new()
            .from((identity.name.clone(), identity.email.clone()))
            .subject(draft.subject.clone())
            .message_id(message_id.to_string())
            .date(date_ms / 1000);
        if !draft.to.is_empty() {
            b = b.to(to_builder_addrs(&draft.to));
        }
        if !draft.cc.is_empty() {
            b = b.cc(to_builder_addrs(&draft.cc));
        }
        if with_bcc && !draft.bcc.is_empty() {
            b = b.bcc(to_builder_addrs(&draft.bcc));
        }
        if let Some(rt) = identity.reply_to.as_deref().filter(|s| !s.trim().is_empty()) {
            b = b.reply_to(rt.to_string());
        }
        if let Some(irt) = &draft.in_reply_to {
            b = b.in_reply_to(irt.clone());
        }
        if !draft.references.is_empty() {
            b = b.references(draft.references.clone());
        }
        b = b.header("User-Agent", mail_builder::headers::raw::Raw::new("WolfMail"));
        b = b.text_body(text.clone());
        if !html.trim().is_empty() {
            b = b.html_body(html.clone());
        }
        for (cid, mime, bytes) in &inline {
            b = b.inline(mime.clone(), cid.clone(), bytes.clone());
        }
        for a in attachments {
            b = b.attachment(a.mime.clone(), a.filename.clone(), a.bytes.clone());
        }
        b.write_to_vec().map_err(|e| ApiError::internal(format!("Could not build the message: {e}")))
    };
    let mut recipients: Vec<String> = draft.to.iter().chain(&draft.cc).chain(&draft.bcc).map(|a| a.email.clone()).collect();
    recipients.sort();
    recipients.dedup();
    Ok(BuiltMessage {
        wire: build(false)?,
        copy: build(true)?,
        message_id: message_id.to_string(),
        envelope_from: identity.email.clone(),
        recipients,
    })
}

pub enum SmtpSecret {
    Password(String),
    OAuthToken(String),
}

fn transport(cfg: &ServerConfig, secret: &SmtpSecret) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    crate::tls::install_crypto_provider();
    let host = cfg.host.trim();
    if host.is_empty() {
        return Err(ApiError::invalid("smtp.host", "Enter the outgoing (SMTP) server name."));
    }
    let tls_params = || {
        TlsParameters::builder(host.to_string())
            .build_rustls()
            .map_err(|e| ApiError::new(ErrorKind::Tls, format!("TLS setup for {host} failed: {e}")))
    };
    let tls = match cfg.security {
        Security::Tls => Tls::Wrapper(tls_params()?),
        Security::Starttls => Tls::Required(tls_params()?),
        Security::None => {
            if !is_localhost(host) {
                return Err(ApiError::invalid(
                    "smtp.security",
                    "Unencrypted connections are only allowed to this computer. Choose SSL/TLS or STARTTLS.",
                ));
            }
            Tls::None
        }
    };
    let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
        .port(cfg.port)
        .tls(tls)
        .timeout(Some(Duration::from_secs(60)))
        // "[127.0.0.1]" rather than the machine's hostname, which would leak into every Received header.
        .hello_name(ClientId::Ipv4(std::net::Ipv4Addr::LOCALHOST));
    match secret {
        SmtpSecret::Password(p) => {
            if !cfg.username.is_empty() {
                b = b.credentials(Credentials::new(cfg.username.clone(), p.clone())).authentication(vec![Mechanism::Plain, Mechanism::Login]);
            }
        }
        SmtpSecret::OAuthToken(t) => {
            b = b.credentials(Credentials::new(cfg.username.clone(), t.clone())).authentication(vec![Mechanism::Xoauth2]);
        }
    }
    Ok(b.build())
}

/// A submission failure and whether trying again later can help (RFC 5321 §4.2.1: 4yz transient, 5yz permanent).
pub struct SmtpFailure {
    pub error: ApiError,
    pub transient: bool,
}

fn map_smtp_err(host: &str, e: lettre::transport::smtp::Error) -> SmtpFailure {
    let msg = e.to_string();
    let lower = msg.to_ascii_lowercase();
    if e.is_permanent() {
        if lower.contains("auth") || lower.contains("535") || lower.contains("credentials") {
            return SmtpFailure { error: ApiError::auth(format!("The outgoing server {host} rejected the sign-in: {msg}")), transient: false };
        }
        return SmtpFailure { error: ApiError::server(format!("{host} refused the message: {msg}")), transient: false };
    }
    if e.is_transient() {
        return SmtpFailure { error: ApiError::server(format!("{host} temporarily refused the message: {msg}")), transient: true };
    }
    if e.is_timeout() {
        return SmtpFailure { error: ApiError::network(format!("Timed out talking to {host}.")), transient: true };
    }
    if lower.contains("tls") || lower.contains("certificate") {
        return SmtpFailure { error: ApiError::new(ErrorKind::Tls, format!("Secure connection to {host} failed: {msg}")), transient: false };
    }
    SmtpFailure { error: ApiError::network(format!("Could not talk to {host}: {msg}")), transient: true }
}

pub async fn send(cfg: &ServerConfig, secret: &SmtpSecret, msg: &BuiltMessage) -> std::result::Result<(), SmtpFailure> {
    let permanent = |error: ApiError| SmtpFailure { error, transient: false };
    let t = transport(cfg, secret).map_err(permanent)?;
    let from: lettre::Address = msg
        .envelope_from
        .parse()
        .map_err(|_| permanent(ApiError::invalid("from", format!("“{}” is not a valid sender address.", msg.envelope_from))))?;
    let to: Vec<lettre::Address> = msg.recipients.iter().filter_map(|r| r.parse().ok()).collect();
    let env = lettre::address::Envelope::new(Some(from), to).map_err(|e| permanent(ApiError::invalid("to", format!("Invalid recipients: {e}"))))?;
    t.send_raw(&env, &msg.wire).await.map(|_| ()).map_err(|e| map_smtp_err(&cfg.host, e))
}

pub async fn test(cfg: &ServerConfig, secret: &SmtpSecret) -> Result<()> {
    let t = transport(cfg, secret)?;
    match t.test_connection().await {
        Ok(true) => Ok(()),
        Ok(false) => Err(ApiError::network(format!("{} did not accept a connection.", cfg.host))),
        Err(e) => Err(map_smtp_err(&cfg.host, e).error),
    }
}

/// Plain mime type from a file extension, for attachments chosen from disk.
pub fn mime_for_path(path: &std::path::Path) -> String {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "ics" => "text/calendar",
        "vcf" => "text/vcard",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "eml" => "message/rfc822",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident() -> Identity {
        Identity {
            id: "i".into(),
            account_id: "a".into(),
            name: "Paul Wolf".into(),
            email: "paul@wolf.example".into(),
            reply_to: None,
            signature_html: String::new(),
            reply_on_top: true,
            is_default: true,
        }
    }

    #[test]
    fn builds_reply_with_bcc_only_in_copy() {
        let d = DraftInput {
            id: None,
            identity_id: "i".into(),
            to: vec![Address { name: Some("Ann".into()), email: "ann@example.com".into() }],
            cc: vec![],
            bcc: vec![Address { name: None, email: "secret@example.com".into() }],
            subject: "Re: Plan".into(),
            html: "<p>Yes <img src=\"data:image/png;base64,iVBORw0KGgo=\"></p>".into(),
            text: String::new(),
            in_reply_to: Some("parent@example.com".into()),
            references: vec!["root@example.com".into(), "parent@example.com".into()],
            reply_to_message_id: None,
            forward_of_message_id: None,
            attachments: vec![],
            send_at: None,
        };
        let att = ResolvedAttachment { filename: "a.txt".into(), mime: "text/plain".into(), bytes: b"hello".to_vec() };
        let m = build_message(&d, &ident(), "mid1@wolf.example", &[att], 1_793_095_200_000, true).unwrap();
        let wire = String::from_utf8_lossy(&m.wire);
        let copy = String::from_utf8_lossy(&m.copy);
        assert!(!wire.to_ascii_lowercase().contains("bcc:"), "{wire}");
        assert!(copy.contains("secret@example.com"));
        assert!(wire.contains("In-Reply-To: <parent@example.com>"), "{wire}");
        assert!(wire.contains("<root@example.com>"));
        assert!(wire.contains("Message-ID: <mid1@wolf.example>"), "{wire}");
        assert!(wire.contains("cid:"), "inline image should be a cid part");
        assert!(!wire.contains("data:image/png"));
        assert_eq!(m.recipients, vec!["ann@example.com", "secret@example.com"]);
        // Round-trip through our own parser.
        let h = crate::mime::parse_headers(&m.wire);
        assert_eq!(h.message_id.as_deref(), Some("mid1@wolf.example"));
        assert_eq!(h.from.email, "paul@wolf.example");
        let b = crate::mime::parse_body(&m.wire, false);
        assert!(b.attachments.iter().any(|a| a.filename == "a.txt"));
        assert!(b.text.unwrap().contains("Yes"));
    }

    #[test]
    fn rejects_bad_address() {
        let mut d = DraftInput {
            id: None,
            identity_id: "i".into(),
            to: vec![Address { name: None, email: "not an address".into() }],
            cc: vec![],
            bcc: vec![],
            subject: "x".into(),
            html: String::new(),
            text: "x".into(),
            in_reply_to: None,
            references: vec![],
            reply_to_message_id: None,
            forward_of_message_id: None,
            attachments: vec![],
            send_at: None,
        };
        let e = build_message(&d, &ident(), "m@x", &[], 0, true).err().unwrap();
        assert_eq!(e.field.as_deref(), Some("to"));
        assert!(build_message(&d, &ident(), "m@x", &[], 0, false).is_ok(), "a draft may hold a half-typed address");
        d.to.clear();
        assert!(build_message(&d, &ident(), "m@x", &[], 0, true).is_err());
        assert!(build_message(&d, &ident(), "m@x", &[], 0, false).is_ok());
    }
}
