//! MIME parsing (mail-parser 1.0), HTML sanitising (ammonia), tracker stripping, plain-text → HTML, snippets.
// Source: docs/API.md MessageFull — `html` is ALREADY SANITISED here; remote resources blocked unless allowed.
use crate::model::{Address, Attachment, AuthResults, ListUnsubscribe};
use base64::Engine as _;
use mail_parser::{MessageParser, PartKind};
use regex::Regex;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// Header-level facts the sync stores per message.
#[derive(Debug, Clone, Default)]
pub struct HeaderInfo {
    pub subject: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    pub date: Option<i64>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub list_unsubscribe: Option<ListUnsubscribe>,
    pub auth_results: Option<AuthResults>,
    pub multipart_mixed: bool,
}

fn addr_list(list: Option<mail_parser::AddressList<'_>>) -> Vec<Address> {
    let Some(list) = list else { return Vec::new() };
    list.mailboxes()
        .filter_map(|m| {
            let email = m.address()?.trim().to_string();
            if email.is_empty() {
                return None;
            }
            let name = m.name().map(|n| n.trim().trim_matches('"').to_string()).filter(|n| !n.is_empty() && *n != email);
            Some(Address { name, email })
        })
        .collect()
}

fn strip_angle(s: &str) -> String {
    s.trim().trim_start_matches('<').trim_end_matches('>').trim().to_string()
}

/// Parse a header block (or a full message) into the per-message facts the list view needs.
pub fn parse_headers(raw: &[u8]) -> HeaderInfo {
    let parser = MessageParser::default();
    let Some(msg) = parser.parse_headers(raw) else { return HeaderInfo::default() };
    let headers = msg.headers();
    let raw_header = |name: &str| -> Option<String> {
        headers.get(name).map(|h| unfold(&String::from_utf8_lossy(h.raw_value())))
    };
    let list_unsubscribe = raw_header("List-Unsubscribe").map(|v| {
        parse_list_unsubscribe(&v, raw_header("List-Unsubscribe-Post").as_deref())
    });
    let auth_results = raw_header("Authentication-Results").map(|v| parse_auth_results(&v));
    let ctype = msg.content_type();
    let multipart_mixed = ctype
        .map(|c| c.ctype().eq_ignore_ascii_case("multipart") && c.subtype().is_some_and(|s| s.eq_ignore_ascii_case("mixed")))
        .unwrap_or(false);
    HeaderInfo {
        subject: msg.subject().unwrap_or("").trim().to_string(),
        from: addr_list(msg.from()).into_iter().next().unwrap_or_default(),
        to: addr_list(msg.to()),
        cc: addr_list(msg.cc()),
        bcc: addr_list(msg.bcc()),
        reply_to: addr_list(msg.reply_to()),
        date: msg.date().map(|d| d.to_timestamp() * 1000),
        message_id: msg.message_id().map(strip_angle).filter(|s| !s.is_empty()),
        in_reply_to: msg.in_reply_to().and_then(|l| l.first().map(strip_angle)).filter(|s| !s.is_empty()),
        references: msg
            .references()
            .map(|l| l.iter().map(strip_angle).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default(),
        list_unsubscribe,
        auth_results,
        multipart_mixed,
    }
}

fn unfold(v: &str) -> String {
    v.replace("\r\n", "").replace('\n', "").trim().to_string()
}

/// Source: RFC 2369 §3.2 (List-Unsubscribe is a comma-separated list of <URI>) and RFC 8058 §3.1
/// (one-click requires `List-Unsubscribe-Post: List-Unsubscribe=One-Click` AND an https URI).
pub fn parse_list_unsubscribe(value: &str, post: Option<&str>) -> ListUnsubscribe {
    let mut out = ListUnsubscribe::default();
    for part in value.split(',') {
        let uri = strip_angle(part);
        let lower = uri.to_ascii_lowercase();
        if lower.starts_with("mailto:") && out.mailto.is_none() {
            out.mailto = Some(uri);
        } else if (lower.starts_with("https://") || lower.starts_with("http://")) && out.http.is_none() {
            out.http = Some(uri);
        }
    }
    let post_ok = post.is_some_and(|p| p.replace(' ', "").eq_ignore_ascii_case("List-Unsubscribe=One-Click"));
    out.one_click = post_ok && out.http.as_deref().is_some_and(|h| h.to_ascii_lowercase().starts_with("https://"));
    out
}

/// Source: RFC 8601 §2.2 — `method=result` tokens inside Authentication-Results.
pub fn parse_auth_results(value: &str) -> AuthResults {
    let find = |method: &str| -> Option<String> {
        let re = Regex::new(&format!(r"(?i)\b{method}\s*=\s*([a-z]+)")).ok()?;
        re.captures(value).map(|c| c[1].to_ascii_lowercase())
    };
    AuthResults { spf: find("spf"), dkim: find("dkim"), dmarc: find("dmarc") }
}

/// Everything the reading pane needs from a full message.
#[derive(Debug, Clone, Default)]
pub struct ParsedBody {
    pub html: Option<String>,
    pub text: Option<String>,
    /// No HTML part: `html` is `text_to_html(text)`.
    pub plain_text: bool,
    pub remote_content_blocked: bool,
    pub trackers_removed: i64,
    pub attachments: Vec<Attachment>,
    pub snippet: String,
    /// Plain text used for the FTS index.
    pub index_text: String,
    pub has_attachments: bool,
}

pub fn parse_body(raw: &[u8], allow_remote: bool) -> ParsedBody {
    let parser = MessageParser::default();
    let Some(msg) = parser.parse(raw) else {
        let text = String::from_utf8_lossy(raw).to_string();
        return ParsedBody {
            html: Some(text_to_html(&text)),
            plain_text: true,
            snippet: make_snippet(&text),
            index_text: text.clone(),
            text: Some(text),
            ..Default::default()
        };
    };

    // Inline images referenced by cid: become data: URIs so nothing is fetched.
    let mut cid_map: HashMap<String, String> = HashMap::new();
    let mut attachments = Vec::new();
    for part in msg.attachments() {
        let ctype = part.content_type();
        let mime = ctype
            .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")).to_ascii_lowercase())
            .unwrap_or_else(|| "application/octet-stream".into());
        let cid = part.content_id().map(strip_angle).filter(|s| !s.is_empty());
        let filename = part
            .attachment_name()
            .map(|s| s.to_string())
            .or_else(|| match part.kind() {
                PartKind::Message(nested) => nested.subject().map(|s| format!("{s}.eml")),
                _ => None,
            })
            .unwrap_or_else(|| default_filename(&mime));
        let inline = part.is_inline() && cid.is_some() && mime.starts_with("image/");
        if let (Some(c), true) = (&cid, mime.starts_with("image/")) {
            let data = part.decoded();
            cid_map.insert(
                c.to_ascii_lowercase(),
                format!("data:{};base64,{}", mime, base64::engine::general_purpose::STANDARD.encode(&data)),
            );
        }
        attachments.push(Attachment {
            id: part.id().to_string(),
            filename,
            mime,
            size: part.decoded_len() as i64,
            inline,
            content_id: cid,
        });
    }

    let text = msg.body_text(0).map(|t| t.to_string());
    let html_src = msg.html_body().next().and_then(|p| match p.kind() {
        PartKind::Html => msg.body_html(0).map(|h| h.to_string()),
        _ => None,
    });
    let (html, blocked, trackers) = match &html_src {
        Some(h) => {
            let s = sanitize_html(h, allow_remote, &cid_map);
            (Some(s.html), s.remote_blocked, s.trackers)
        }
        None => (text.as_deref().map(text_to_html), 0, 0),
    };
    // An inline image that the HTML actually shows is not an attachment the user needs to see in the list.
    let referenced: HashSet<String> = match &html_src {
        Some(h) => cid_refs(h),
        None => HashSet::new(),
    };
    for a in attachments.iter_mut() {
        if let Some(c) = &a.content_id {
            if referenced.contains(&c.to_ascii_lowercase()) {
                a.inline = true;
            }
        }
    }
    let plain_for_index = text.clone().unwrap_or_else(|| html_src.as_deref().map(html_to_text).unwrap_or_default());
    let has_attachments = attachments.iter().any(|a| !a.inline);
    ParsedBody {
        snippet: make_snippet(&plain_for_index),
        index_text: plain_for_index,
        html,
        text,
        plain_text: html_src.is_none(),
        remote_content_blocked: blocked > 0,
        trackers_removed: trackers as i64,
        attachments,
        has_attachments,
    }
}

/// Decoded bytes of one attachment (`id` is the mail-parser part id, as returned in `Attachment::id`).
pub fn attachment_bytes(raw: &[u8], id: &str) -> Option<(String, String, Vec<u8>)> {
    let msg = MessageParser::default().parse(raw)?;
    let pid: u32 = id.parse().ok()?;
    let part = msg.part(pid)?;
    let ctype = part.content_type();
    let mime = ctype
        .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")).to_ascii_lowercase())
        .unwrap_or_else(|| "application/octet-stream".into());
    let name = part.attachment_name().map(|s| s.to_string()).unwrap_or_else(|| default_filename(&mime));
    let bytes = match part.kind() {
        // A forwarded message is saved as its original RFC 822 bytes.
        PartKind::Message(_) => part.decoded().into_owned(),
        _ => part.decoded().into_owned(),
    };
    Some((name, mime, bytes))
}

fn default_filename(mime: &str) -> String {
    let ext = match mime {
        "application/pdf" => "pdf",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "text/calendar" => "ics",
        "message/rfc822" => "eml",
        "text/plain" => "txt",
        "text/html" => "html",
        _ => "bin",
    };
    format!("attachment.{ext}")
}

fn cid_refs(html: &str) -> HashSet<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"(?i)cid:([^"'\s>)]+)"#).unwrap());
    re.captures_iter(html).map(|c| c[1].to_ascii_lowercase()).collect()
}

/// Hosts whose only purpose in an email is to record that it was opened. Source: the senders' own
/// documentation of their open-tracking domains (Mailchimp list-manage, SendGrid, HubSpot, Mailgun, Mixpanel,
/// Yesware, Mailtrack, Superhuman, Streak, Litmus, Sendinblue/Brevo, Customer.io, Intercom, Salesforce/ExactTarget)
/// — a pixel from these is stripped even when remote content is allowed.
const TRACKER_HOSTS: &[&str] = &[
    "list-manage.com/track",
    "mailchimp.com/track",
    "sendgrid.net/wf/open",
    "track.hubspot.com",
    "t.hubspotemail.net",
    "email.mg.",
    "api.mixpanel.com/track",
    "t.yesware.com",
    "mailtrack.io",
    "r.superhuman.com",
    "mailfoogae.appspot.com",
    "streak.com/track",
    "emltrk.com",
    "litmus.com/r/",
    "sendibt",
    "sendibm",
    "customeriomail.com/e/o",
    "via.intercom.io",
    "exct.net/open",
    "intelligentwolf.com/api/email/track/o/",
];

pub struct Sanitized {
    pub html: String,
    pub remote_blocked: usize,
    pub trackers: usize,
}

/// Find `<img>` tags that are tracking pixels: 1×1 (or 0×0) by attribute or inline style, display:none, or from a
/// known tracker host. Returns their src values.
fn tracker_srcs(html: &str) -> HashSet<String> {
    static IMG: OnceLock<Regex> = OnceLock::new();
    static SRC: OnceLock<Regex> = OnceLock::new();
    static TINY: OnceLock<Regex> = OnceLock::new();
    let img = IMG.get_or_init(|| Regex::new(r"(?is)<img\b[^>]*>").unwrap());
    let src = SRC.get_or_init(|| Regex::new(r#"(?is)\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).unwrap());
    let tiny = TINY.get_or_init(|| {
        Regex::new(r#"(?is)(\b(width|height)\s*=\s*["']?\s*[01](px)?\s*["']?(\s|/|>))|((width|height)\s*:\s*[01](px)?\s*(;|"|'))|(display\s*:\s*none)|(visibility\s*:\s*hidden)"#).unwrap()
    });
    let mut out = HashSet::new();
    for m in img.find_iter(html) {
        let tag = m.as_str();
        let Some(c) = src.captures(tag) else { continue };
        let s = c.get(1).or(c.get(2)).or(c.get(3)).map(|x| html_escape::decode_html_entities(x.as_str()).to_string()).unwrap_or_default();
        if s.is_empty() || s.starts_with("cid:") || s.starts_with("data:") {
            continue;
        }
        let lower = s.to_ascii_lowercase();
        let tiny_count = tiny.find_iter(tag).count();
        if tiny_count >= 2 || tiny.find_iter(tag).any(|x| x.as_str().to_ascii_lowercase().contains("display") || x.as_str().to_ascii_lowercase().contains("visibility")) || TRACKER_HOSTS.iter().any(|h| lower.contains(h)) {
            out.insert(s);
        }
    }
    out
}

/// Remove remote url(...) and @import from <style> blocks and style attributes when remote content is blocked.
fn strip_remote_css(css: &str, count: &AtomicUsize) -> String {
    static URL: OnceLock<Regex> = OnceLock::new();
    static IMPORT: OnceLock<Regex> = OnceLock::new();
    let url = URL.get_or_init(|| Regex::new(r#"(?is)url\(\s*['"]?\s*(https?:)?//[^)]*\)"#).unwrap());
    let import = IMPORT.get_or_init(|| Regex::new(r#"(?is)@import[^;]*;?"#).unwrap());
    let mut n = 0;
    let a = import.replace_all(css, |_: &regex::Captures| {
        n += 1;
        ""
    });
    let b = url.replace_all(&a, |_: &regex::Captures| {
        n += 1;
        "none"
    });
    count.fetch_add(n, Ordering::Relaxed);
    b.into_owned()
}

pub fn sanitize_html(src: &str, allow_remote: bool, cid_map: &HashMap<String, String>) -> Sanitized {
    let trackers = Arc::new(tracker_srcs(src));
    let tracker_count = Arc::new(AtomicUsize::new(0));
    let blocked = Arc::new(AtomicUsize::new(0));

    // <style> contents are kept (email layout depends on them) but scrubbed of remote loads first.
    static STYLE: OnceLock<Regex> = OnceLock::new();
    let style_re = STYLE.get_or_init(|| Regex::new(r"(?is)(<style\b[^>]*>)(.*?)(</style>)").unwrap());
    let pre: Cow<str> = if allow_remote {
        Cow::Borrowed(src)
    } else {
        let b = Arc::clone(&blocked);
        Cow::Owned(
            style_re
                .replace_all(src, |c: &regex::Captures| format!("{}{}{}", &c[1], strip_remote_css(&c[2], &b), &c[3]))
                .into_owned(),
        )
    };

    let mut builder = ammonia::Builder::default();
    builder
        .add_tags(&["style", "font", "center", "big", "small", "u", "s", "strike", "tt", "section", "header", "footer", "article", "main", "nav", "figure", "figcaption", "mark", "picture", "wbr"])
        .rm_clean_content_tags(&["style"])
        .add_generic_attributes(&["style", "align", "valign", "bgcolor", "width", "height", "dir", "lang", "class", "id", "title", "role", "border", "color"])
        .add_tag_attributes("font", &["face", "size", "color"])
        .add_tag_attributes("table", &["cellpadding", "cellspacing", "border", "background", "summary"])
        .add_tag_attributes("td", &["colspan", "rowspan", "nowrap", "background"])
        .add_tag_attributes("th", &["colspan", "rowspan", "nowrap", "background"])
        .add_tag_attributes("img", &["src", "alt", "width", "height"])
        .add_url_schemes(&["data", "cid"])
        .id_prefix(Some("wm-"))
        .link_rel(Some("noopener noreferrer"))
        .strip_comments(true);
    let cid_map = Arc::new(cid_map.clone());
    {
        let trackers = Arc::clone(&trackers);
        let tracker_count = Arc::clone(&tracker_count);
        let blocked = Arc::clone(&blocked);
        builder.attribute_filter(move |element, attribute, value| {
            match (element, attribute) {
                ("img", "src") | (_, "background") => {
                    let v = value.trim();
                    let lower = v.to_ascii_lowercase();
                    if lower.starts_with("cid:") {
                        let key = v[4..].trim_matches(|c| c == '<' || c == '>').to_ascii_lowercase();
                        return cid_map.get(&key).map(|d| Cow::Owned(d.clone()));
                    }
                    if lower.starts_with("data:image/") {
                        return Some(Cow::Borrowed(value));
                    }
                    if lower.starts_with("data:") {
                        return None;
                    }
                    let decoded = html_escape::decode_html_entities(v).to_string();
                    if element == "img" && (trackers.contains(v) || trackers.contains(&decoded)) {
                        tracker_count.fetch_add(1, Ordering::Relaxed);
                        return None;
                    }
                    if !allow_remote {
                        blocked.fetch_add(1, Ordering::Relaxed);
                        return None;
                    }
                    Some(Cow::Borrowed(value))
                }
                (_, "style") if !allow_remote && value.to_ascii_lowercase().contains("url(") => {
                    Some(Cow::Owned(strip_remote_css(value, &blocked)))
                }
                _ => Some(Cow::Borrowed(value)),
            }
        });
    }
    let html = builder.clean(&pre).to_string();
    Sanitized { html, remote_blocked: blocked.load(Ordering::Relaxed), trackers: tracker_count.load(Ordering::Relaxed) }
}

/// Plain text → safe HTML: escaped, links made clickable, quoted lines (`>`) shown as blockquotes.
pub fn text_to_html(text: &str) -> String {
    let finder = linkify::LinkFinder::new();
    let mut out = String::from("<div class=\"wm-plain\" style=\"white-space:pre-wrap;word-wrap:break-word;font-family:inherit\">");
    let mut quote_depth = 0usize;
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        let depth = line.chars().take_while(|c| *c == '>' || *c == ' ').filter(|c| *c == '>').count();
        while quote_depth < depth {
            out.push_str("<blockquote style=\"margin:0 0 0 .5em;padding-left:.75em;border-left:3px solid #c41e1e55\">");
            quote_depth += 1;
        }
        while quote_depth > depth {
            out.push_str("</blockquote>");
            quote_depth -= 1;
        }
        let content = if depth > 0 { line.trim_start_matches(['>', ' ']) } else { line };
        let mut last = 0;
        for link in finder.links(content) {
            out.push_str(&html_escape::encode_text(&content[last..link.start()]));
            let href = match link.kind() {
                linkify::LinkKind::Email => format!("mailto:{}", link.as_str()),
                _ => link.as_str().to_string(),
            };
            out.push_str(&format!(
                "<a href=\"{}\" rel=\"noopener noreferrer\">{}</a>",
                html_escape::encode_double_quoted_attribute(&href),
                html_escape::encode_text(link.as_str())
            ));
            last = link.end();
        }
        out.push_str(&html_escape::encode_text(&content[last..]));
        out.push('\n');
    }
    while quote_depth > 0 {
        out.push_str("</blockquote>");
        quote_depth -= 1;
    }
    out.push_str("</div>");
    out
}

/// Crude HTML → text for snippets and the search index (not for display).
pub fn html_to_text(html: &str) -> String {
    static DROP: OnceLock<Regex> = OnceLock::new();
    static BR: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    let drop = DROP.get_or_init(|| Regex::new(r"(?is)<(style|script|head|title)\b.*?</\s*(style|script|head|title)\s*>").unwrap());
    let br = BR.get_or_init(|| Regex::new(r"(?i)<(br|/p|/div|/tr|/li|/h[1-6])\b[^>]*>").unwrap());
    let tag = TAG.get_or_init(|| Regex::new(r"(?s)<[^>]*>").unwrap());
    let a = drop.replace_all(html, " ");
    let b = br.replace_all(&a, "\n");
    let c = tag.replace_all(&b, "");
    html_escape::decode_html_entities(&c).to_string()
}

/// First ~200 characters of what the sender actually wrote: quoted history and "On … wrote:" lines skipped.
pub fn make_snippet(text: &str) -> String {
    static WROTE: OnceLock<Regex> = OnceLock::new();
    let wrote = WROTE.get_or_init(|| Regex::new(r"(?i)^\s*(on\b.*\bwrote:|-{2,}\s*original message\s*-{2,}|from:\s.*)$").unwrap());
    let mut out = String::new();
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('>') {
            continue;
        }
        if wrote.is_match(l) {
            break;
        }
        if l == "--" || l == "-- " {
            break;
        }
        if l.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(l);
        if out.chars().count() > 220 {
            break;
        }
    }
    let collapsed: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(200).collect()
}

/// Subject with reply/forward prefixes removed (for threading fallback and "Re:" building).
pub fn base_subject(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)^\s*((re|fw|fwd|aw|wg|sv|vs|antw|ref|tr|rif|r)(\[\d+\])?\s*:\s*)+").unwrap());
    let mut cur = s.trim().to_string();
    loop {
        let next = re.replace(&cur, "").trim().to_string();
        if next == cur {
            break;
        }
        cur = next;
    }
    cur
}

pub fn is_reply_subject(s: &str) -> bool {
    base_subject(s) != s.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MSG: &[u8] = b"From: \"Ann Example\" <ann@example.com>\r\n\
To: Bob <bob@example.org>, carol@example.net\r\n\
Subject: =?utf-8?q?Caf=C3=A9_plans?=\r\n\
Date: Tue, 27 Oct 2026 10:00:00 +0000\r\n\
Message-ID: <abc@example.com>\r\n\
In-Reply-To: <parent@example.com>\r\n\
References: <root@example.com> <parent@example.com>\r\n\
List-Unsubscribe: <mailto:unsub@example.com>, <https://example.com/u?x=1>\r\n\
List-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n\
Authentication-Results: mx.example.org; spf=pass smtp.mailfrom=example.com; dkim=pass header.d=example.com; dmarc=fail\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b1\"\r\n\
\r\n\
--b1\r\n\
Content-Type: multipart/related; boundary=\"b2\"\r\n\
\r\n\
--b2\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><head><style>body{background:url(https://evil.example/bg.png)} @import url(https://evil.example/x.css);</style></head>\
<body><p onclick=\"alert(1)\">Hello <b>Bob</b></p><script>alert(2)</script>\
<img src=\"cid:logo@x\"><img src=\"https://cdn.example.com/photo.jpg\" width=\"300\">\
<img src=\"https://t.example.com/open.gif\" width=\"1\" height=\"1\">\
<img src=\"https://x.list-manage.com/track/open.php?u=1\">\
<form action=\"https://phish\"><input name=p></form><a href=\"javascript:alert(3)\">bad</a><a href=\"https://ok.example\">ok</a></body></html>\r\n\
--b2\r\n\
Content-Type: image/png\r\n\
Content-ID: <logo@x>\r\n\
Content-Disposition: inline\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
iVBORw0KGgo=\r\n\
--b2--\r\n\
--b1\r\n\
Content-Type: application/pdf; name=\"invoice.pdf\"\r\n\
Content-Disposition: attachment; filename=\"invoice.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQK\r\n\
--b1--\r\n";

    #[test]
    fn headers_parse() {
        let h = parse_headers(MSG);
        assert_eq!(h.subject, "Café plans");
        assert_eq!(h.from.email, "ann@example.com");
        assert_eq!(h.from.name.as_deref(), Some("Ann Example"));
        assert_eq!(h.to.len(), 2);
        assert_eq!(h.to[1].email, "carol@example.net");
        assert_eq!(h.message_id.as_deref(), Some("abc@example.com"));
        assert_eq!(h.in_reply_to.as_deref(), Some("parent@example.com"));
        assert_eq!(h.references, vec!["root@example.com", "parent@example.com"]);
        assert_eq!(h.date, Some(1793095200000));
        let lu = h.list_unsubscribe.unwrap();
        assert_eq!(lu.mailto.as_deref(), Some("mailto:unsub@example.com"));
        assert_eq!(lu.http.as_deref(), Some("https://example.com/u?x=1"));
        assert!(lu.one_click);
        let ar = h.auth_results.unwrap();
        assert_eq!(ar.spf.as_deref(), Some("pass"));
        assert_eq!(ar.dmarc.as_deref(), Some("fail"));
        assert!(h.multipart_mixed);
    }

    #[test]
    fn body_sanitised_blocked() {
        let b = parse_body(MSG, false);
        let html = b.html.unwrap();
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("onclick"), "{html}");
        assert!(!html.contains("javascript:"), "{html}");
        assert!(!html.contains("<form"), "{html}");
        assert!(!html.contains("evil.example"), "{html}");
        assert!(!html.contains("cdn.example.com"), "{html}");
        assert!(!html.contains("t.example.com"), "{html}");
        assert!(html.contains("data:image/png;base64,"), "{html}");
        assert!(html.contains("https://ok.example"), "{html}");
        assert!(html.contains("<style>"), "{html}");
        assert!(b.remote_content_blocked);
        assert_eq!(b.trackers_removed, 2);
        let pdf = b.attachments.iter().find(|a| a.filename == "invoice.pdf").unwrap();
        assert!(!pdf.inline);
        assert_eq!(pdf.mime, "application/pdf");
        let logo = b.attachments.iter().find(|a| a.content_id.as_deref() == Some("logo@x")).unwrap();
        assert!(logo.inline);
        assert!(b.has_attachments);
        assert!(b.snippet.starts_with("Hello Bob"), "{}", b.snippet);
        let (name, mime, bytes) = attachment_bytes(MSG, &pdf.id).unwrap();
        assert_eq!(name, "invoice.pdf");
        assert_eq!(mime, "application/pdf");
        assert_eq!(&bytes[..5], b"%PDF-");
    }

    #[test]
    fn body_remote_allowed_still_strips_trackers() {
        let b = parse_body(MSG, true);
        let html = b.html.unwrap();
        assert!(html.contains("cdn.example.com/photo.jpg"));
        assert!(!html.contains("t.example.com/open.gif"));
        assert!(!html.contains("list-manage.com"));
        assert_eq!(b.trackers_removed, 2);
        assert!(!b.remote_content_blocked);
    }

    #[test]
    fn plain_text_rendering() {
        let h = text_to_html("Hi <you>\nsee https://example.com/a?b=1&c=2\n> quoted\n>> deeper\nmail me@x.org");
        assert!(h.contains("Hi &lt;you&gt;"));
        assert!(h.contains("<a href=\"https://example.com/a?b=1&amp;c=2\""));
        assert!(h.contains("<blockquote"));
        assert!(h.contains("mailto:me@x.org"));
        assert_eq!(h.matches("<blockquote").count(), h.matches("</blockquote>").count());
    }

    #[test]
    fn snippet_skips_quotes() {
        let s = make_snippet("Thanks, that works.\n\nOn Tue, Ann wrote:\n> old stuff\n");
        assert_eq!(s, "Thanks, that works.");
    }

    #[test]
    fn subjects() {
        assert_eq!(base_subject("Re: Fwd: RE: Hello"), "Hello");
        assert_eq!(base_subject("AW: Angebot"), "Angebot");
        assert!(is_reply_subject("Re: x"));
        assert!(!is_reply_subject("Report"));
    }

    #[test]
    fn one_click_requires_https() {
        let lu = parse_list_unsubscribe("<http://example.com/u>", Some("List-Unsubscribe=One-Click"));
        assert!(!lu.one_click);
    }
}
