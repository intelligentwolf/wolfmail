//! Import accounts from Thunderbird / Betterbird profiles (read-only; never touches logins.json or key4.db).
//!
//! Sources:
//! - comm-central mailnews/base/public/MailNewsTypes2.idl — nsMsgSocketType: plain=0, alwaysSTARTTLS=2, SSL=3;
//!   nsMsgAuthMethod: none=1, old=2, passwordCleartext=3, passwordEncrypted=4, GSSAPI=5, NTLM=6, External=7,
//!   secure=8, anything=9, OAuth2=10.
//! - comm-central mailnews/mailnews.js — defaults when a pref is absent: mail.server.default.port=-1 (protocol
//!   default), mail.server.default.authMethod=3, mail.server.default.socketType=0,
//!   mail.smtpserver.default.authMethod=3, mail.smtpserver.default.try_ssl=0,
//!   mail.identity.default.reply_on_top=0 ("0=bottom 1=top 2=select").
//! - Protocol default ports: IMAP 143 (RFC 3501 §2.1), IMAPS 993 and submissions 465 (RFC 8314 §7.3), submission
//!   587 (RFC 6409 §3.1).
use crate::error::{ApiError, Result};
use crate::model::*;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq)]
pub enum PrefValue {
    Str(String),
    Int(i64),
    Bool(bool),
}

pub fn parse_prefs(src: &str) -> HashMap<String, PrefValue> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"^\s*user_pref\(\s*"((?:[^"\\]|\\.)*)"\s*,\s*(.*?)\s*\)\s*;\s*$"#).unwrap());
    let mut out = HashMap::new();
    for line in src.lines() {
        let Some(c) = re.captures(line) else { continue };
        let key: String = serde_json::from_str(&format!("\"{}\"", &c[1])).unwrap_or_else(|_| c[1].to_string());
        let raw = c[2].trim();
        let val = if raw.starts_with('"') {
            match serde_json::from_str::<String>(raw) {
                Ok(s) => PrefValue::Str(s),
                Err(_) => PrefValue::Str(raw.trim_matches('"').to_string()),
            }
        } else if raw == "true" {
            PrefValue::Bool(true)
        } else if raw == "false" {
            PrefValue::Bool(false)
        } else if let Ok(n) = raw.parse::<i64>() {
            PrefValue::Int(n)
        } else {
            continue;
        };
        out.insert(key, val);
    }
    out
}

struct Prefs(HashMap<String, PrefValue>);

impl Prefs {
    fn s(&self, k: &str) -> Option<String> {
        match self.0.get(k)? {
            PrefValue::Str(s) => Some(s.clone()),
            PrefValue::Int(n) => Some(n.to_string()),
            PrefValue::Bool(b) => Some(b.to_string()),
        }
    }
    fn i(&self, k: &str) -> Option<i64> {
        match self.0.get(k)? {
            PrefValue::Int(n) => Some(*n),
            PrefValue::Str(s) => s.parse().ok(),
            PrefValue::Bool(b) => Some(*b as i64),
        }
    }
    fn b(&self, k: &str) -> Option<bool> {
        match self.0.get(k)? {
            PrefValue::Bool(b) => Some(*b),
            PrefValue::Int(n) => Some(*n != 0),
            PrefValue::Str(s) => Some(s == "true"),
        }
    }
    fn list(&self, k: &str) -> Vec<String> {
        self.s(k).map(|v| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default()
    }
}

fn security_from_socket(v: i64) -> Option<Security> {
    match v {
        3 => Some(Security::Tls),
        2 => Some(Security::Starttls),
        0 => Some(Security::None),
        _ => None,
    }
}

fn provider_for(host: &str) -> Provider {
    let h = host.to_ascii_lowercase();
    if h == "imap.gmail.com" || h == "imap.googlemail.com" {
        Provider::Gmail
    } else if h == "outlook.office365.com" || h.ends_with(".outlook.com") || h == "imap-mail.outlook.com" {
        Provider::Outlook
    } else {
        Provider::Imap
    }
}

fn auth_for(method: i64, provider: Provider) -> AuthKind {
    match (method, provider) {
        (10, Provider::Gmail) => AuthKind::OauthGoogle,
        (10, Provider::Outlook) => AuthKind::OauthMicrosoft,
        _ => AuthKind::Password,
    }
}

fn text_sig_to_html(t: &str) -> String {
    html_escape::encode_text(t).replace('\n', "<br>")
}

fn dir_stats(dir: &Path) -> (i64, i64) {
    let mut folders = 0;
    let mut bytes = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(e.path());
            } else {
                bytes += md.len() as i64;
                if e.path().extension().is_some_and(|x| x == "msf") {
                    folders += 1;
                }
            }
        }
    }
    (folders, bytes)
}

fn resolve_dir(profile: &Path, prefs: &Prefs, srv: &str) -> Option<PathBuf> {
    if let Some(rel) = prefs.s(&format!("mail.server.{srv}.directory-rel")) {
        if let Some(r) = rel.strip_prefix("[ProfD]") {
            return Some(profile.join(r));
        }
    }
    prefs.s(&format!("mail.server.{srv}.directory")).map(PathBuf::from)
}

/// Read one profile's prefs.js into importable accounts.
/// WolfMail keeps one account per address, but a Thunderbird profile can hold the same address twice — e.g. an old
/// password mailbox and the current Google Workspace sign-in — and the import kept the first and silently dropped the
/// Google one (found 28 Oct 2026). Keep one per address, preferring a Google/Microsoft
/// sign-in account (the provider login is the live mailbox; a duplicate password server is usually a leftover),
/// otherwise the first in Thunderbird's own account order.
fn dedupe_by_address(accounts: Vec<ImportAccount>) -> Vec<ImportAccount> {
    let mut out: Vec<ImportAccount> = Vec::new();
    for a in accounts {
        match out.iter().position(|x| x.email.eq_ignore_ascii_case(&a.email)) {
            None => out.push(a),
            Some(i) => {
                if out[i].provider == Provider::Imap && a.provider != Provider::Imap {
                    out[i] = a;
                }
            }
        }
    }
    out
}

pub fn read_profile(profile: &Path) -> Result<ImportCandidate> {
    let src = std::fs::read_to_string(profile.join("prefs.js"))
        .map_err(|e| ApiError::not_found(&format!("prefs.js in {} ({e})", profile.display())))?;
    let prefs = Prefs(parse_prefs(&src));
    let default_smtp = prefs.s("mail.smtp.defaultserver");
    let mut accounts = Vec::new();
    for acct in prefs.list("mail.accountmanager.accounts") {
        let Some(srv) = prefs.s(&format!("mail.account.{acct}.server")) else { continue };
        if prefs.s(&format!("mail.server.{srv}.type")).as_deref() != Some("imap") {
            continue;
        }
        let host = prefs.s(&format!("mail.server.{srv}.hostname")).unwrap_or_default();
        let real_host = prefs.s(&format!("mail.server.{srv}.realhostname")).unwrap_or(host.clone());
        let provider = provider_for(&real_host);
        let socket = prefs.i(&format!("mail.server.{srv}.socketType")).unwrap_or(0);
        let Some(security) = security_from_socket(socket) else { continue };
        let port = match prefs.i(&format!("mail.server.{srv}.port")).filter(|p| *p > 0) {
            Some(p) => p as u16,
            None => {
                if security == Security::Tls {
                    993
                } else {
                    143
                }
            }
        };
        let auth = auth_for(prefs.i(&format!("mail.server.{srv}.authMethod")).unwrap_or(3), provider);
        let username = prefs
            .s(&format!("mail.server.{srv}.realuserName"))
            .or_else(|| prefs.s(&format!("mail.server.{srv}.userName")))
            .unwrap_or_default();
        let imap = ServerConfig { host: real_host.clone(), port, security, username: username.clone(), auth };

        let mut identities = Vec::new();
        let mut smtp_key: Option<String> = None;
        for (n, id) in prefs.list(&format!("mail.account.{acct}.identities")).iter().enumerate() {
            let Some(email) = prefs.s(&format!("mail.identity.{id}.useremail")).filter(|e| e.contains('@')) else { continue };
            if n == 0 {
                smtp_key = prefs.s(&format!("mail.identity.{id}.smtpServer"));
            }
            let html_fmt = prefs.b(&format!("mail.identity.{id}.htmlSigFormat")).unwrap_or(false);
            let mut sig = prefs.s(&format!("mail.identity.{id}.htmlSigText")).unwrap_or_default();
            if prefs.b(&format!("mail.identity.{id}.attach_signature")).unwrap_or(false) {
                if let Some(path) = prefs.s(&format!("mail.identity.{id}.sig_file")) {
                    if let Ok(file) = std::fs::read_to_string(&path) {
                        let is_html = path.to_ascii_lowercase().ends_with(".html") || path.to_ascii_lowercase().ends_with(".htm");
                        sig = if is_html { file } else { text_sig_to_html(&file) };
                    }
                }
            } else if !html_fmt && !sig.is_empty() {
                sig = text_sig_to_html(&sig);
            }
            identities.push(ImportIdentity {
                name: prefs.s(&format!("mail.identity.{id}.fullName")).unwrap_or_default(),
                email: email.clone(),
                reply_to: prefs.s(&format!("mail.identity.{id}.reply_to")).filter(|s| !s.is_empty()),
                signature_html: sig,
                reply_on_top: prefs.i(&format!("mail.identity.{id}.reply_on_top")).unwrap_or(0) == 1,
                is_default: n == 0,
            });
        }
        let Some(primary) = identities.first().map(|i| i.email.clone()) else { continue };
        let smtp_key = smtp_key.or(default_smtp.clone());
        let smtp = match smtp_key {
            Some(k) => {
                let sec = security_from_socket(prefs.i(&format!("mail.smtpserver.{k}.try_ssl")).unwrap_or(0)).unwrap_or(Security::Starttls);
                let shost = prefs.s(&format!("mail.smtpserver.{k}.hostname")).unwrap_or_default();
                let sport = prefs.i(&format!("mail.smtpserver.{k}.port")).filter(|p| *p > 0).map(|p| p as u16).unwrap_or(if sec == Security::Tls { 465 } else { 587 });
                ServerConfig {
                    host: shost,
                    port: sport,
                    security: sec,
                    username: prefs.s(&format!("mail.smtpserver.{k}.username")).unwrap_or_else(|| username.clone()),
                    auth: auth_for(prefs.i(&format!("mail.smtpserver.{k}.authMethod")).unwrap_or(3), provider),
                }
            }
            None => ServerConfig { host: String::new(), port: 587, security: Security::Starttls, username: username.clone(), auth: imap.auth },
        };
        let (folder_count, local_size_bytes) = resolve_dir(profile, &prefs, &srv).map(|d| dir_stats(&d)).unwrap_or((0, 0));
        let name = prefs.s(&format!("mail.server.{srv}.name")).filter(|n| !n.is_empty()).unwrap_or(primary.clone());
        accounts.push(ImportAccount { name, email: primary, provider, imap, smtp, identities, folder_count, local_size_bytes });
    }
    let mut tags: Vec<Tag> = prefs
        .0
        .keys()
        .filter_map(|k| k.strip_prefix("mailnews.tags.").and_then(|r| r.strip_suffix(".tag")).map(|s| s.to_string()))
        .map(|key| Tag {
            name: prefs.s(&format!("mailnews.tags.{key}.tag")).unwrap_or_default(),
            color: prefs.s(&format!("mailnews.tags.{key}.color")).unwrap_or_else(|| "#808080".into()),
            key,
        })
        .collect();
    tags.sort_by(|a, b| a.key.cmp(&b.key));
    let accounts = dedupe_by_address(accounts);
    let lower = profile.to_string_lossy().to_ascii_lowercase();
    Ok(ImportCandidate {
        source: if lower.contains("betterbird") { "betterbird".into() } else { "thunderbird".into() },
        profile_path: profile.to_string_lossy().to_string(),
        accounts,
        tags,
    })
}

/// Profile base directories Thunderbird and Betterbird use on each platform (native and Flatpak installs).
pub fn base_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
        for rel in [
            ".thunderbird",
            ".betterbird",
            ".var/app/eu.betterbird.Betterbird/.thunderbird",
            ".var/app/eu.betterbird.Betterbird/.betterbird",
            ".var/app/org.mozilla.Thunderbird/.thunderbird",
            "snap/thunderbird/common/.thunderbird",
            "Library/Thunderbird",
            "Library/Betterbird",
        ] {
            v.push(home.join(rel));
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        v.push(PathBuf::from(&appdata).join("Thunderbird"));
        v.push(PathBuf::from(&appdata).join("Betterbird"));
    }
    v
}

/// Profile directories listed in a base dir's profiles.ini.
pub fn profiles_in(base: &Path) -> Vec<PathBuf> {
    let Ok(ini) = std::fs::read_to_string(base.join("profiles.ini")) else { return Vec::new() };
    let mut out = Vec::new();
    let mut path: Option<String> = None;
    let mut relative = true;
    let flush = |path: &mut Option<String>, relative: bool, out: &mut Vec<PathBuf>| {
        if let Some(p) = path.take() {
            let full = if relative { base.join(&p) } else { PathBuf::from(&p) };
            if full.join("prefs.js").exists() && !out.contains(&full) {
                out.push(full);
            }
        }
    };
    for line in ini.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            flush(&mut path, relative, &mut out);
            relative = true;
        } else if let Some(p) = l.strip_prefix("Path=") {
            path = Some(p.to_string());
        } else if let Some(r) = l.strip_prefix("IsRelative=") {
            relative = r.trim() == "1";
        }
    }
    flush(&mut path, relative, &mut out);
    out
}

pub fn scan(profile_path: Option<&str>) -> Result<Vec<ImportCandidate>> {
    if let Some(p) = profile_path {
        return Ok(vec![read_profile(Path::new(p))?]);
    }
    let mut out = Vec::new();
    for base in base_dirs() {
        for prof in profiles_in(&base) {
            if let Ok(c) = read_profile(&prof) {
                if !c.accounts.is_empty() {
                    out.push(c);
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFS: &str = r##"
user_pref("mail.accountmanager.accounts", "account1,account2,account3");
user_pref("mail.account.account1.server", "server1");
user_pref("mail.account.account1.identities", "id1,id2");
user_pref("mail.account.account2.server", "server2");
user_pref("mail.account.account2.identities", "id3");
user_pref("mail.account.account3.server", "server3");
user_pref("mail.server.server1.type", "imap");
user_pref("mail.server.server1.hostname", "imap.gmail.com");
user_pref("mail.server.server1.port", 993);
user_pref("mail.server.server1.socketType", 3);
user_pref("mail.server.server1.authMethod", 10);
user_pref("mail.server.server1.userName", "me@gmail.com");
user_pref("mail.server.server1.name", "Personal \"Gmail\"");
user_pref("mail.server.server2.type", "imap");
user_pref("mail.server.server2.hostname", "mail.example.co.uk");
user_pref("mail.server.server2.socketType", 2);
user_pref("mail.server.server2.userName", "a@example.co.uk");
user_pref("mail.server.server3.type", "none");
user_pref("mail.identity.id1.useremail", "me@gmail.com");
user_pref("mail.identity.id1.fullName", "Me Myself");
user_pref("mail.identity.id1.smtpServer", "smtp1");
user_pref("mail.identity.id1.htmlSigFormat", true);
user_pref("mail.identity.id1.htmlSigText", "<b>Me</b>");
user_pref("mail.identity.id1.reply_on_top", 1);
user_pref("mail.identity.id2.useremail", "alias@gmail.com");
user_pref("mail.identity.id3.useremail", "a@example.co.uk");
user_pref("mail.identity.id3.htmlSigText", "Line 1\nLine <2>");
user_pref("mail.smtp.defaultserver", "smtp2");
user_pref("mail.smtpserver.smtp1.hostname", "smtp.gmail.com");
user_pref("mail.smtpserver.smtp1.port", 465);
user_pref("mail.smtpserver.smtp1.try_ssl", 3);
user_pref("mail.smtpserver.smtp1.authMethod", 10);
user_pref("mail.smtpserver.smtp2.hostname", "smtp.example.co.uk");
user_pref("mail.smtpserver.smtp2.try_ssl", 2);
user_pref("mailnews.tags.$label1.tag", "Important");
user_pref("mailnews.tags.$label1.color", "#FF0000");
"##;

    #[test]
    fn parses_synthetic_profile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("prefs.js"), PREFS).unwrap();
        let c = read_profile(dir.path()).unwrap();
        assert_eq!(c.accounts.len(), 2);
        let g = &c.accounts[0];
        assert_eq!(g.name, "Personal \"Gmail\"");
        assert_eq!(g.provider, Provider::Gmail);
        assert_eq!(g.imap.auth, AuthKind::OauthGoogle);
        assert_eq!(g.imap.security, Security::Tls);
        assert_eq!(g.smtp.host, "smtp.gmail.com");
        assert_eq!(g.smtp.port, 465);
        assert_eq!(g.identities.len(), 2);
        assert!(g.identities[0].reply_on_top);
        assert_eq!(g.identities[0].signature_html, "<b>Me</b>");
        let e = &c.accounts[1];
        assert_eq!(e.imap.port, 143);
        assert_eq!(e.imap.security, Security::Starttls);
        assert_eq!(e.imap.auth, AuthKind::Password);
        assert_eq!(e.smtp.host, "smtp.example.co.uk", "falls back to mail.smtp.defaultserver");
        assert_eq!(e.smtp.port, 587);
        assert_eq!(e.identities[0].signature_html, "Line 1<br>Line &lt;2&gt;");
        assert_eq!(c.tags[0].name, "Important");
    }

    /// Read-only check against the real Betterbird profile copy in the session scratchpad, when present.
    /// (The copy is not committed: it contains real addresses.)
    #[test]
    fn real_profile_copy_if_present() {
        let Ok(p) = std::env::var("WOLFMAIL_TEST_PREFS") else { return };
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(&p, dir.path().join("prefs.js")).unwrap();
        let c = read_profile(dir.path()).unwrap();
        assert_eq!(c.accounts.len(), 11, "Paul's profile has 11 IMAP accounts");
        assert_eq!(c.accounts.iter().filter(|a| a.provider == Provider::Gmail).count(), 2);
        assert_eq!(c.accounts.iter().filter(|a| a.provider == Provider::Outlook).count(), 1);
        assert_eq!(c.accounts.iter().filter(|a| a.imap.host == "imap.stackmail.com").count(), 7);
        assert!(c.accounts.iter().all(|a| !a.smtp.host.is_empty()));
        assert!(c.accounts.iter().all(|a| a.identities.iter().all(|i| i.reply_on_top)));
        assert_eq!(c.tags.len(), 5);
    }
}
