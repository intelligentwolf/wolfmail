//! Account autodetection, in Thunderbird's order: built-in table → Mozilla ISPDB → the domain's own autoconfig →
//! MX-based lookup → SRV (RFC 6186) → guess.
//!
//! Sources:
//! - Mozilla ISPDB / autoconfig format: https://wiki.mozilla.org/Thunderbird:Autoconfiguration:ConfigFileFormat
//!   (`clientConfig/emailProvider/incomingServer|outgoingServer`, socketType SSL|STARTTLS|plain,
//!   %EMAILADDRESS% / %EMAILLOCALPART% / %EMAILDOMAIN% placeholders), served at
//!   https://autoconfig.thunderbird.net/v1.1/<domain>.
//! - Built-in values below are copied from the ISPDB entries fetched 2026-10-28 (gmail.com, outlook.com,
//!   yahoo.com, icloud.com), Fastmail's "Server names and ports" help page, and 20i's StackMail settings as
//!   recorded in IntelligentWolf's business-email settings (docs.20i.com thunderbird settings).
//! - StackMail's MX host is `mx.stackmail.com` (public DNS: the MX of a StackMail-hosted domain answers
//!   `10 mx.stackmail.com.`, checked 2026-10-28).
//! - RFC 6186 §3: `_imaps._tcp`, `_imap._tcp`, `_submissions._tcp` (RFC 8314 §5.1), `_submission._tcp`.
use crate::error::{ApiError, Result};
use crate::model::{AuthKind, AutodetectResult, Provider, Security, ServerConfig};
use serde::Deserialize;
use std::time::Duration;

fn sc(host: &str, port: u16, security: Security, username: &str, auth: AuthKind) -> ServerConfig {
    ServerConfig { host: host.into(), port, security, username: username.into(), auth }
}

fn gmail(email: &str) -> AutodetectResult {
    AutodetectResult {
        provider: Provider::Gmail,
        imap: sc("imap.gmail.com", 993, Security::Tls, email, AuthKind::OauthGoogle),
        smtp: sc("smtp.gmail.com", 465, Security::Tls, email, AuthKind::OauthGoogle),
        source: "builtin".into(),
    }
}

fn microsoft(email: &str) -> AutodetectResult {
    AutodetectResult {
        provider: Provider::Outlook,
        imap: sc("outlook.office365.com", 993, Security::Tls, email, AuthKind::OauthMicrosoft),
        smtp: sc("smtp-mail.outlook.com", 587, Security::Starttls, email, AuthKind::OauthMicrosoft),
        source: "builtin".into(),
    }
}

fn stackmail(email: &str) -> AutodetectResult {
    AutodetectResult {
        provider: Provider::Imap,
        imap: sc("imap.stackmail.com", 993, Security::Tls, email, AuthKind::Password),
        smtp: sc("smtp.stackmail.com", 587, Security::Starttls, email, AuthKind::Password),
        source: "builtin".into(),
    }
}

pub fn builtin(email: &str) -> Option<AutodetectResult> {
    let domain = email.rsplit_once('@')?.1.to_ascii_lowercase();
    let pw = |imap: (&str, u16, Security), smtp: (&str, u16, Security)| AutodetectResult {
        provider: Provider::Imap,
        imap: sc(imap.0, imap.1, imap.2, email, AuthKind::Password),
        smtp: sc(smtp.0, smtp.1, smtp.2, email, AuthKind::Password),
        source: "builtin".into(),
    };
    Some(match domain.as_str() {
        "gmail.com" | "googlemail.com" => gmail(email),
        d if d == "outlook.com" || d == "msn.com" || d.starts_with("hotmail.") || d.starts_with("live.") || d.starts_with("outlook.") => microsoft(email),
        "yahoo.com" | "ymail.com" | "rocketmail.com" => pw(("imap.mail.yahoo.com", 993, Security::Tls), ("smtp.mail.yahoo.com", 465, Security::Tls)),
        "icloud.com" | "me.com" | "mac.com" => pw(("imap.mail.me.com", 993, Security::Tls), ("smtp.mail.me.com", 587, Security::Starttls)),
        "fastmail.com" | "fastmail.fm" => pw(("imap.fastmail.com", 993, Security::Tls), ("smtp.fastmail.com", 465, Security::Tls)),
        _ => return None,
    })
}

/// Hosted-mail recognised from the domain's MX records (Google Workspace, Microsoft 365, 20i StackMail).
pub fn from_mx(email: &str, mx_hosts: &[String]) -> Option<AutodetectResult> {
    for mx in mx_hosts {
        let m = mx.trim_end_matches('.').to_ascii_lowercase();
        if m.ends_with(".google.com") || m.ends_with(".googlemail.com") {
            return Some(AutodetectResult { source: "mx".into(), ..gmail(email) });
        }
        if m.ends_with(".mail.protection.outlook.com") || m.ends_with(".outlook.com") {
            return Some(AutodetectResult { source: "mx".into(), ..microsoft(email) });
        }
        if m.ends_with(".stackmail.com") || m == "stackmail.com" {
            return Some(AutodetectResult { source: "mx".into(), ..stackmail(email) });
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct ClientConfig {
    #[serde(rename = "emailProvider")]
    email_provider: EmailProvider,
}

#[derive(Debug, Deserialize)]
struct EmailProvider {
    #[serde(rename = "incomingServer", default)]
    incoming: Vec<XmlServer>,
    #[serde(rename = "outgoingServer", default)]
    outgoing: Vec<XmlServer>,
}

#[derive(Debug, Deserialize)]
struct XmlServer {
    #[serde(rename = "@type")]
    kind: String,
    hostname: String,
    port: u16,
    #[serde(rename = "socketType", default)]
    socket_type: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(rename = "authentication", default)]
    authentication: Vec<String>,
}

fn substitute(template: &str, email: &str) -> String {
    let (local, domain) = email.split_once('@').unwrap_or((email, ""));
    template.replace("%EMAILADDRESS%", email).replace("%EMAILLOCALPART%", local).replace("%EMAILDOMAIN%", domain)
}

fn to_config(s: &XmlServer, email: &str, provider: Provider) -> Option<ServerConfig> {
    let security = match s.socket_type.as_deref().map(|x| x.to_ascii_uppercase()).as_deref() {
        Some("SSL") => Security::Tls,
        Some("STARTTLS") => Security::Starttls,
        // Plain-text servers are never offered: the UI cannot even save one for a remote host.
        _ => return None,
    };
    let oauth = s.authentication.iter().any(|a| a.eq_ignore_ascii_case("OAuth2"));
    let auth = match (provider, oauth) {
        (Provider::Gmail, true) => AuthKind::OauthGoogle,
        (Provider::Outlook, true) => AuthKind::OauthMicrosoft,
        _ => AuthKind::Password,
    };
    Some(ServerConfig {
        host: substitute(&s.hostname, email),
        port: s.port,
        security,
        username: substitute(s.username.as_deref().unwrap_or("%EMAILADDRESS%"), email),
        auth,
    })
}

pub fn parse_autoconfig_xml(xml: &str, email: &str, source: &str) -> Option<AutodetectResult> {
    let cfg: ClientConfig = quick_xml::de::from_str(xml).ok()?;
    let imap = cfg.email_provider.incoming.iter().find(|s| s.kind.eq_ignore_ascii_case("imap") && to_config(s, email, Provider::Imap).is_some())?;
    let smtp = cfg.email_provider.outgoing.iter().find(|s| s.kind.eq_ignore_ascii_case("smtp") && to_config(s, email, Provider::Imap).is_some())?;
    let host = imap.hostname.to_ascii_lowercase();
    let provider = if host.ends_with("gmail.com") {
        Provider::Gmail
    } else if host.ends_with("office365.com") || host.ends_with("outlook.com") {
        Provider::Outlook
    } else {
        Provider::Imap
    };
    Some(AutodetectResult {
        provider,
        imap: to_config(imap, email, provider)?,
        smtp: to_config(smtp, email, provider)?,
        source: source.into(),
    })
}

async fn fetch_text(http: &reqwest::Client, url: &str) -> Option<String> {
    let r = http.get(url).timeout(Duration::from_secs(8)).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.text().await.ok()
}

fn guess(email: &str, domain: &str) -> AutodetectResult {
    AutodetectResult {
        provider: Provider::Imap,
        imap: sc(&format!("imap.{domain}"), 993, Security::Tls, email, AuthKind::Password),
        smtp: sc(&format!("smtp.{domain}"), 587, Security::Starttls, email, AuthKind::Password),
        source: "guess".into(),
    }
}

/// Registrable-ish base of an MX host ("mx1.mail.example.co.uk" → "example.co.uk") for the ISPDB lookup.
fn mx_base(host: &str) -> String {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    let parts: Vec<&str> = h.split('.').collect();
    if parts.len() <= 2 {
        return h;
    }
    let two_level = ["co.uk", "org.uk", "ac.uk", "com.au", "net.au", "co.nz", "co.jp", "com.br", "co.za"];
    let last2 = parts[parts.len() - 2..].join(".");
    let n = if two_level.contains(&last2.as_str()) { 3 } else { 2 };
    parts[parts.len().saturating_sub(n)..].join(".")
}

pub async fn autodetect(http: &reqwest::Client, email: &str) -> Result<AutodetectResult> {
    let email = email.trim();
    let Some((_, domain)) = email.rsplit_once('@').filter(|(l, d)| !l.is_empty() && d.contains('.')) else {
        return Err(ApiError::invalid("email", "Enter a full email address, like name@example.com."));
    };
    let domain = domain.to_ascii_lowercase();
    if let Some(r) = builtin(email) {
        return Ok(r);
    }
    if let Some(x) = fetch_text(http, &format!("https://autoconfig.thunderbird.net/v1.1/{domain}")).await {
        if let Some(r) = parse_autoconfig_xml(&x, email, "ispdb") {
            return Ok(r);
        }
    }
    let own = format!("https://autoconfig.{domain}/mail/config-v1.1.xml?emailaddress={}", urlencode(email));
    if let Some(x) = fetch_text(http, &own).await {
        if let Some(r) = parse_autoconfig_xml(&x, email, "autoconfig") {
            return Ok(r);
        }
    }
    let resolver = hickory_resolver::Resolver::builder_tokio().ok().and_then(|b| b.build().ok());
    if let Some(res) = &resolver {
        if let Ok(mx) = res.mx_lookup(format!("{domain}.")).await {
            let mut hosts: Vec<(u16, String)> = mx
                .answers()
                .iter()
                .filter_map(|r| match &r.data {
                    hickory_resolver::proto::rr::RData::MX(m) => Some((m.preference, m.exchange.to_utf8())),
                    _ => None,
                })
                .collect();
            hosts.sort();
            let names: Vec<String> = hosts.into_iter().map(|(_, h)| h).collect();
            if let Some(r) = from_mx(email, &names) {
                return Ok(r);
            }
            if let Some(first) = names.first() {
                let base = mx_base(first);
                if base != domain {
                    if let Some(x) = fetch_text(http, &format!("https://autoconfig.thunderbird.net/v1.1/{base}")).await {
                        if let Some(mut r) = parse_autoconfig_xml(&x, email, "ispdb") {
                            r.source = "mx".into();
                            return Ok(r);
                        }
                    }
                }
            }
        }
        let srv = |name: String| async move {
            let l = res.srv_lookup(name).await.ok()?;
            l.answers().iter().find_map(|r| match &r.data {
                hickory_resolver::proto::rr::RData::SRV(s) if s.target.to_utf8() != "." => {
                    Some((s.target.to_utf8().trim_end_matches('.').to_string(), s.port))
                }
                _ => None,
            })
        };
        let imap = match srv(format!("_imaps._tcp.{domain}.")).await {
            Some((h, p)) => Some(sc(&h, p, Security::Tls, email, AuthKind::Password)),
            None => srv(format!("_imap._tcp.{domain}.")).await.map(|(h, p)| sc(&h, p, Security::Starttls, email, AuthKind::Password)),
        };
        let smtp = match srv(format!("_submissions._tcp.{domain}.")).await {
            Some((h, p)) => Some(sc(&h, p, Security::Tls, email, AuthKind::Password)),
            None => srv(format!("_submission._tcp.{domain}.")).await.map(|(h, p)| sc(&h, p, Security::Starttls, email, AuthKind::Password)),
        };
        if let (Some(imap), Some(smtp)) = (imap, smtp) {
            return Ok(AutodetectResult { provider: Provider::Imap, imap, smtp, source: "srv".into() });
        }
    }
    Ok(guess(email, &domain))
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from https://autoconfig.thunderbird.net/v1.1/gmail.com (fetched 2026-10-28).
    const GMAIL_XML: &str = r#"<clientConfig version="1.1">
  <emailProvider id="googlemail.com">
    <domain>gmail.com</domain>
    <displayName>Google Mail</displayName>
    <incomingServer type="imap">
      <hostname>imap.gmail.com</hostname>
      <port>993</port>
      <socketType>SSL</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>OAuth2</authentication>
      <authentication>password-cleartext</authentication>
    </incomingServer>
    <incomingServer type="pop3">
      <hostname>pop.gmail.com</hostname>
      <port>995</port>
      <socketType>SSL</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>OAuth2</authentication>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.gmail.com</hostname>
      <port>465</port>
      <socketType>SSL</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>OAuth2</authentication>
    </outgoingServer>
  </emailProvider>
</clientConfig>"#;

    #[test]
    fn parses_ispdb() {
        let r = parse_autoconfig_xml(GMAIL_XML, "me@gmail.com", "ispdb").unwrap();
        assert_eq!(r.provider, Provider::Gmail);
        assert_eq!(r.imap.host, "imap.gmail.com");
        assert_eq!(r.imap.security, Security::Tls);
        assert_eq!(r.imap.auth, AuthKind::OauthGoogle);
        assert_eq!(r.smtp.port, 465);
        assert_eq!(r.smtp.username, "me@gmail.com");
    }

    #[test]
    fn plain_servers_are_refused() {
        let xml = r#"<clientConfig><emailProvider id="x"><incomingServer type="imap"><hostname>mail.x.org</hostname><port>143</port><socketType>plain</socketType><username>%EMAILLOCALPART%</username><authentication>password-cleartext</authentication></incomingServer><outgoingServer type="smtp"><hostname>mail.x.org</hostname><port>25</port><socketType>plain</socketType></outgoingServer></emailProvider></clientConfig>"#;
        assert!(parse_autoconfig_xml(xml, "a@x.org", "ispdb").is_none());
    }

    #[test]
    fn builtin_and_mx() {
        assert_eq!(builtin("a@gmail.com").unwrap().provider, Provider::Gmail);
        assert_eq!(builtin("a@hotmail.co.uk").unwrap().provider, Provider::Outlook);
        assert_eq!(builtin("a@fastmail.com").unwrap().smtp.port, 465);
        assert!(builtin("a@example-business.test").is_none());
        let r = from_mx("someone@example-business.test", &["mx.stackmail.com.".into()]).unwrap();
        assert_eq!(r.imap.host, "imap.stackmail.com");
        assert_eq!(r.smtp.security, Security::Starttls);
        assert_eq!(from_mx("x@corp.com", &["corp-com.mail.protection.outlook.com.".into()]).unwrap().provider, Provider::Outlook);
        assert_eq!(from_mx("x@biz.com", &["aspmx.l.google.com.".into()]).unwrap().provider, Provider::Gmail);
    }

    #[test]
    fn mx_bases() {
        assert_eq!(mx_base("mx1.mail.example.co.uk."), "example.co.uk");
        assert_eq!(mx_base("mx.hoster.net"), "hoster.net");
    }
}
