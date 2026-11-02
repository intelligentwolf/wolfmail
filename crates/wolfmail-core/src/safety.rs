//! Spoofing and fraud checks on received mail (Paul, 2 Nov 2026: "scan emails in wolfmail for spoofing etc e.g. an
//! email that has a company name but doesn't come from the domain, an invoice that doesn't match etc").
//!
//! Plain rules, run on this computer, nothing sent anywhere:
//! - the name says a company or a bank, the address is somewhere else (a brand's own domains, a company name, a free
//!   webmail address, an address written into the name);
//! - a domain made to look like one this account deals with (wo1f.uk.com, wolf-uk.com, Cyrillic letters);
//! - the receiving server's own SPF / DKIM / DMARC verdict (RFC 8601 Authentication-Results, parsed in mime.rs);
//! - replies that go to another domain;
//! - links whose words name one site and whose address is another, links to bare IP addresses, short links in mail
//!   asking for a login or payment;
//! - bank details different from the ones earlier mail from the same domain carried, a first email asking for money,
//!   attachments that run code or pretend to be a document.
//!
//! `analyze` is pure. `AccountCtx` / `MessageCtx` are read from the store; `scan_batch` stores each message's level
//! (messages.risk) for the list and the "Looks fake" view; `assess` gives the reading pane its warnings.

use crate::error::Result;
use crate::mime;
use crate::model::{Address, Attachment, AuthResults};
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// messages.risk: the stored level. messages.risk_checked: 0 = not yet, 1 = headers only, 2 = with the body.
pub const LEVEL_NONE: i64 = 0;
pub const LEVEL_CAUTION: i64 = 1;
pub const LEVEL_DANGER: i64 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Warning {
    /// "danger" or "caution".
    pub level: String,
    pub code: String,
    pub title: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Safety {
    /// 0 none, 1 caution, 2 danger.
    pub level: i64,
    pub warnings: Vec<Warning>,
    /// The person marked this sender as fine ("This sender is fine"); only bank-detail and attachment warnings stay.
    pub trusted: bool,
    /// The body was looked at too (false: only the headers were on this computer).
    pub body_checked: bool,
}

/// What this account is: its own domains, the domains and names of people it writes to.
#[derive(Debug, Clone, Default)]
pub struct AccountCtx {
    pub own_domains: HashSet<String>,
    /// Registrable domains of everyone this account has written to, and its own.
    pub known_domains: HashSet<String>,
    /// A person's name as written (lower case, spaces collapsed) → the addresses it is known at.
    pub known_names: HashMap<String, Vec<String>>,
}

/// What is known about this sender before this message.
#[derive(Debug, Clone, Default)]
pub struct MessageCtx {
    /// The account has written to them, or had mail from them before this message.
    pub sender_known: bool,
    pub trusted: bool,
    /// Bank details seen in earlier mail from the sender's domain: (detail, unix ms first seen).
    pub earlier_bank: Vec<(String, i64)>,
}

pub struct Input<'a> {
    pub from: &'a Address,
    pub reply_to: &'a [Address],
    pub auth: Option<&'a AuthResults>,
    pub bulk: bool,
    pub subject: &'a str,
    /// The body as text (mime ParsedBody.index_text), "" when not on this computer.
    pub text: &'a str,
    /// The body's sanitised HTML (links keep their href), None for plain text or no body.
    pub html: Option<&'a str>,
    pub attachments: &'a [Attachment],
    pub has_body: bool,
}

// ── domains ──────────────────────────────────────────────────────────────────

/// Two-label public suffixes common in UK and international business mail, so "acme.co.uk" and "wolf.uk.com" are one
/// organisation's domain rather than "co.uk" / "uk.com". A short list, not the whole Public Suffix List: a domain under a
/// suffix not listed is taken as its last two labels.
const TWO_LEVEL: &[&str] = &[
    "co.uk", "org.uk", "me.uk", "ltd.uk", "plc.uk", "net.uk", "ac.uk", "gov.uk", "nhs.uk", "sch.uk", "police.uk", "mod.uk",
    "uk.com", "uk.net", "gb.com", "gb.net", "eu.com", "us.com", "co.nz", "org.nz", "net.nz", "com.au", "net.au", "org.au",
    "edu.au", "gov.au", "co.za", "org.za", "com.br", "co.jp", "co.in", "co.kr", "com.cn", "com.mx", "com.sg", "com.hk",
    "co.il", "com.tr", "co.id", "com.my", "com.ph", "com.ar", "com.pk", "com.ng", "co.ke", "com.ua",
];

/// "mail.acme.co.uk" → "acme.co.uk"; "wolf.uk.com" → "wolf.uk.com"; "x.example.com" → "example.com".
pub fn registrable(host: &str) -> String {
    let h = host.trim().trim_end_matches('.').to_lowercase();
    let labels: Vec<&str> = h.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    let last2 = labels[labels.len() - 2..].join(".");
    let take = if TWO_LEVEL.contains(&last2.as_str()) { 3 } else { 2 };
    labels[labels.len() - take..].join(".")
}

pub fn domain_of(email: &str) -> String {
    email.rsplit_once('@').map(|(_, d)| d.trim().trim_end_matches('>').to_lowercase()).unwrap_or_default()
}

/// Free webmail: anyone can make an address there, so a company's name on it proves nothing.
const FREEMAIL: &[&str] = &[
    "gmail.com", "googlemail.com", "outlook.com", "hotmail.com", "hotmail.co.uk", "live.com", "live.co.uk", "msn.com",
    "yahoo.com", "yahoo.co.uk", "ymail.com", "aol.com", "aol.co.uk", "icloud.com", "me.com", "mac.com", "gmx.com",
    "gmx.co.uk", "gmx.net", "gmx.de", "mail.com", "proton.me", "protonmail.com", "pm.me", "yandex.com", "yandex.ru",
    "zoho.com", "zohomail.com", "tutanota.com", "tuta.io", "btinternet.com", "sky.com", "talktalk.net", "virginmedia.com",
    "mail.ru", "qq.com", "163.com",
];

pub fn is_freemail(domain: &str) -> bool {
    FREEMAIL.contains(&domain.to_lowercase().as_str())
}

/// Brands that phishing borrows (matched as whole words in the sender's name, shown with their own spelling), and the domains their mail really comes from (the sender's domain must be one of these
/// or below one). Only brands whose sending domains are well established; anything else falls to the company-name rule.
const BRANDS: &[(&str, &str, &[&str])] = &[
    ("paypal", "PayPal", &["paypal.com", "paypal.co.uk"]),
    ("hsbc", "HSBC", &["hsbc.co.uk", "hsbc.com"]),
    ("barclays", "Barclays", &["barclays.co.uk", "barclays.com", "barclaycard.co.uk"]),
    ("barclaycard", "Barclaycard", &["barclaycard.co.uk", "barclays.co.uk"]),
    ("natwest", "NatWest", &["natwest.com"]),
    ("santander", "Santander", &["santander.co.uk"]),
    ("halifax", "Halifax", &["halifax.co.uk"]),
    ("monzo", "Monzo", &["monzo.com"]),
    ("hmrc", "HMRC", &["gov.uk"]),
    ("dvla", "DVLA", &["gov.uk"]),
    ("royal mail", "Royal Mail", &["royalmail.com", "royalmail.co.uk"]),
    ("dhl", "DHL", &["dhl.com", "dhl.co.uk"]),
    ("docusign", "DocuSign", &["docusign.com", "docusign.net"]),
    ("microsoft", "Microsoft", &["microsoft.com", "microsoftonline.com", "office.com", "office365.com"]),
    ("office 365", "Office 365", &["microsoft.com", "microsoftonline.com", "office.com", "office365.com"]),
    ("apple", "Apple", &["apple.com", "icloud.com"]),
    ("icloud", "iCloud", &["apple.com", "icloud.com"]),
    ("amazon", "Amazon", &["amazon.co.uk", "amazon.com", "amazon.de", "amazon.fr", "amazon.it", "amazon.es", "amazonaws.com"]),
    ("netflix", "Netflix", &["netflix.com"]),
    ("google", "Google", &["google.com"]),
    ("stripe", "Stripe", &["stripe.com"]),
    ("dropbox", "Dropbox", &["dropbox.com", "dropboxmail.com"]),
    ("linkedin", "LinkedIn", &["linkedin.com"]),
    ("ebay", "eBay", &["ebay.co.uk", "ebay.com"]),
];

fn under(domain: &str, base: &str) -> bool {
    domain == base || domain.ends_with(&format!(".{base}"))
}

/// Words that say a name is an organisation's, and words that say nothing about which one.
const ORG_MARKERS: &[&str] = &["ltd", "limited", "plc", "llc", "llp", "inc", "gmbh", "corp", "corporation", "bank", "group"];
const ROLE_WORDS: &[&str] = &["accounts", "finance", "billing", "payroll", "invoices", "invoicing", "payments", "treasury"];
const FILLER: &[&str] = &[
    "the", "and", "of", "for", "team", "dept", "department", "services", "service", "support", "uk", "co", "company", "from",
    "via", "on", "behalf", "ltd", "limited", "plc", "llc", "llp", "inc", "gmbh", "corp", "corporation", "group", "accounts",
    "finance", "billing", "payroll", "invoices", "invoicing", "payments", "treasury", "office", "admin", "customer",
];

fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Letters that look alike, and punctuation that is easy to miss, folded so "wo1f-uk.com" and "wolf.uk.com" compare equal.
fn skeleton(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        let m = match c {
            '0' | 'о' | 'ο' => 'o',
            '1' | '|' | 'і' | 'i' | 'ӏ' | 'ι' => 'l',
            'а' | 'α' => 'a',
            'е' | 'ε' => 'e',
            'р' | 'ρ' => 'p',
            'с' => 'c',
            'у' => 'y',
            'х' | 'χ' => 'x',
            'ѕ' => 's',
            'ԁ' => 'd',
            'ν' => 'v',
            '5' => 's',
            '-' | '.' | '_' => continue,
            other => other,
        };
        out.push(m);
    }
    out.replace("rn", "m").replace("vv", "w").replace("cl", "d")
}

/// One letter added, dropped, or two neighbours swapped ("wolff", "wlof"): the typing slips a squatter registers.
fn one_slip(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a == b {
        return false;
    }
    if a.len() == b.len() {
        let diff: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
        return diff.len() == 2 && diff[1] == diff[0] + 1 && a[diff[0]] == b[diff[1]] && a[diff[1]] == b[diff[0]];
    }
    let (long, short) = if a.len() > b.len() { (&a, &b) } else { (&b, &a) };
    if long.len() != short.len() + 1 {
        return false;
    }
    let mut i = 0;
    while i < short.len() && long[i] == short[i] {
        i += 1;
    }
    long[i + 1..] == short[i..]
}

/// The known domain `sender` was made to look like, if any.
pub fn lookalike_of<'a>(sender: &str, known: impl IntoIterator<Item = &'a String>) -> Option<String> {
    let s = registrable(sender);
    if s.is_empty() {
        return None;
    }
    let sk = skeleton(&s);
    let label = s.split('.').next().unwrap_or("");
    for k in known {
        if *k == s || k.is_empty() {
            continue;
        }
        let kl = k.split('.').next().unwrap_or("");
        let same_skeleton = skeleton(k) == sk;
        // A slip in the name part, same ending: "wolff.uk.com" for "wolf.uk.com". Short names collide by chance.
        // (Both at least six letters: "mail.com"/"gmail.com" and "xero.com"/"xerox.com" are different companies.)
        let slip = kl.chars().count() >= 6 && label.chars().count() >= 6 && s.strip_prefix(label) == k.strip_prefix(kl) && one_slip(label, kl);
        if same_skeleton || slip {
            return Some(k.clone());
        }
    }
    None
}

/// A domain written with non-Latin letters or in its punycode form.
fn exotic_domain(domain: &str) -> bool {
    domain.split('.').any(|l| l.starts_with("xn--")) || !domain.is_ascii()
}

// ── body ─────────────────────────────────────────────────────────────────────

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("safety regex"))
}

fn payment_context(subject: &str, text: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = re(&R, r"(?i)\b(invoices?|remittance|payment|pay now|bank details|account details|sort code|iban|swift|bacs|wire transfer|overdue|outstanding balance|amount due|direct debit|proforma)\b");
    r.is_match(subject) || r.is_match(text)
}

fn login_context(subject: &str, text: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = re(&R, r"(?i)\b(password|log ?in|sign ?in|verify your|confirm your|account (?:is |has been )?(?:suspended|locked|on hold|disabled)|unusual (?:activity|sign-in)|mailbox (?:is )?full)\b");
    r.is_match(subject) || r.is_match(text)
}

fn says_bank_changed(text: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, r"(?i)\b(new bank (?:details|account)|bank (?:details|account) (?:have|has) (?:changed|been updated)|changed (?:our|my) bank|updated? (?:our |my )?bank (?:details|account)|change (?:of|in) bank (?:details|account))\b")
        .is_match(text)
}

/// The email asks the reader to pay or send money — not a receipt or a newsletter that mentions payment.
fn asks_for_payment(subject: &str, text: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = re(&R, r"(?i)\b(overdue|outstanding (?:balance|amount|invoice|payment)|payment (?:is )?(?:due|required|request(?:ed)?)|please (?:pay|make (?:a |the )?payment|settle|transfer|remit)|bank details|remittance|wire transfer|unpaid invoice|invoice (?:is )?attached|attached invoice|proforma|pay (?:the|this|your) invoice|pay (?:to|into|by bank)|sort code|account number|iban|final (?:notice|reminder))\b");
    r.is_match(subject) || r.is_match(text)
}

/// Words written with letters borrowed from other alphabets or maths symbols to look Latin ("Y𝗈ur L𝗂cenc𝖾", "Аnnuаl"):
/// a word that mixes Latin letters with Cyrillic or Greek ones, or uses the Mathematical Alphanumeric Symbols block.
pub fn disguised_text(s: &str) -> bool {
    s.split(|c: char| !c.is_alphanumeric()).any(|w| {
        let math = w.chars().any(|c| ('\u{1D400}'..='\u{1D7FF}').contains(&c));
        let latin = w.chars().any(|c| c.is_ascii_alphabetic());
        let other = w.chars().any(|c| ('\u{0370}'..='\u{03FF}').contains(&c) || ('\u{0400}'..='\u{04FF}').contains(&c));
        math || (latin && other)
    })
}

/// Click-tracking redirects: the real destination is hidden in the redirect, so a link through one cannot be judged by its
/// host. Outlook Safe Links and Google's redirect carry the destination and are unwrapped instead (`real_target`).
const TRACKERS: &[&str] = &[
    "awstrack.me", "list-manage.com", "sendgrid.net", "mandrillapp.com", "mailchi.mp", "hubspotlinks.com", "convertkit-mail.com",
    "mailgun.org", "sparkpostmail.com", "klclick.com", "rs6.net", "createsend.com",
];

/// A link's real destination: Microsoft 365 Safe Links (…safelinks.protection.outlook.com/?url=…) and Google's
/// redirect (www.google.com/url?q=…) wrap every link in some mailboxes; the wrapped address is what the link opens.
fn real_target(u: url::Url) -> url::Url {
    let host = u.host_str().unwrap_or("").to_lowercase();
    let key = if host.ends_with("safelinks.protection.outlook.com") { Some("url") } else if (host == "www.google.com" || host == "google.com") && u.path() == "/url" { Some("q") } else { None };
    if let Some(k) = key {
        if let Some((_, v)) = u.query_pairs().find(|(n, _)| n == k) {
            if let Ok(inner) = url::Url::parse(&v) {
                return inner;
            }
        }
    }
    u
}

/// ISO 13616 IBAN check: the first four characters moved to the end, letters as 10–35, the number mod 97 is 1.
pub fn iban_valid(iban: &str) -> bool {
    let s: String = iban.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
    if s.len() < 15 || s.len() > 34 || !s.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let (head, tail) = s.split_at(4);
    let mut rem: u32 = 0;
    for c in tail.chars().chain(head.chars()) {
        let v = if c.is_ascii_digit() { c.to_digit(10).unwrap_or(0) } else { c as u32 - 'A' as u32 + 10 };
        rem = if v >= 10 { (rem * 100 + v) % 97 } else { (rem * 10 + v) % 97 };
    }
    rem == 1
}

/// Bank details written in a body: "IBAN:GB82WEST12345698765432", "SORT:123456", "ACCT:12345678".
pub fn bank_details(text: &str) -> Vec<String> {
    static IBAN: OnceLock<Regex> = OnceLock::new();
    static SORT: OnceLock<Regex> = OnceLock::new();
    static ACCT: OnceLock<Regex> = OnceLock::new();
    let mut out: Vec<String> = Vec::new();
    let mut push = |d: String| {
        if !out.contains(&d) {
            out.push(d)
        }
    };
    for m in re(&IBAN, r"(?i)\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]){11,30}\b").find_iter(text) {
        // The match can run on into the next word ("… 7654 32 THANKS"): the longest prefix that passes the check is it.
        let s: String = m.as_str().chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
        if let Some(n) = (15..=s.len()).rev().find(|&n| iban_valid(&s[..n])) {
            push(format!("IBAN:{}", &s[..n]));
        }
    }
    for c in re(&SORT, r"(?i)sort[\s-]*code[^0-9]{0,20}(\d{2})[\s-]?(\d{2})[\s-]?(\d{2})\b").captures_iter(text) {
        push(format!("SORT:{}{}{}", &c[1], &c[2], &c[3]));
    }
    for c in re(&ACCT, r"(?i)account\s*(?:number|no\.?|#)[^0-9]{0,20}(\d{8})\b").captures_iter(text) {
        push(format!("ACCT:{}", &c[1]));
    }
    out
}

/// "SORT:123456" → "sort code 12-34-56", for the warning's words.
pub fn bank_label(d: &str) -> String {
    match d.split_once(':') {
        Some(("SORT", v)) if v.len() == 6 => format!("sort code {}-{}-{}", &v[0..2], &v[2..4], &v[4..6]),
        Some(("ACCT", v)) => format!("account number {v}"),
        Some(("IBAN", v)) => format!("IBAN {v}"),
        _ => d.to_string(),
    }
}

/// (shown words, href) of each link in sanitised HTML.
fn anchors(html: &str) -> Vec<(String, String)> {
    static A: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    let a = re(&A, r#"(?is)<a\b[^>]*?\bhref\s*=\s*"([^"]*)"[^>]*>(.*?)</a>"#);
    let tag = re(&TAG, r"(?s)<[^>]*>");
    a.captures_iter(html)
        .map(|c| {
            let text = html_escape::decode_html_entities(&tag.replace_all(&c[2], "")).trim().to_string();
            (text, html_escape::decode_html_entities(&c[1]).to_string())
        })
        .collect()
}

/// The host a link's words name: "www.hsbc.co.uk", "https://paypal.com/login" — or None for ordinary words.
fn shown_host(text: &str) -> Option<String> {
    static H: OnceLock<Regex> = OnceLock::new();
    let r = re(&H, r"(?i)^(?:https?://)?((?:[a-z0-9-]+\.)+[a-z]{2,})(?:[/:?#]\S*)?$");
    r.captures(text.trim()).map(|c| c[1].to_lowercase())
}

const SHORTENERS: &[&str] = &[
    "bit.ly", "tinyurl.com", "t.co", "goo.gl", "ow.ly", "is.gd", "buff.ly", "rebrand.ly", "cutt.ly", "shorturl.at", "rb.gy", "t.ly", "tiny.cc",
];

/// Files that run code when opened, and documents that carry macros.
const RUNS_CODE: &[&str] = &[
    "exe", "scr", "bat", "cmd", "com", "pif", "js", "jse", "vbs", "vbe", "wsf", "wsh", "hta", "msi", "msix", "appx", "ps1",
    "jar", "lnk", "iso", "img", "vhd", "vhdx", "reg", "cpl", "html", "htm", "shtml", "xhtml", "svg",
];
const MACRO_DOCS: &[&str] = &["docm", "xlsm", "pptm", "dotm", "xltm", "xlam"];
const DOC_LIKE: &[&str] = &["pdf", "doc", "docx", "xls", "xlsx", "jpg", "jpeg", "png", "txt", "csv", "zip"];

fn ext_of(name: &str) -> (String, Option<String>) {
    let parts: Vec<String> = name.trim().to_lowercase().split('.').map(str::to_string).collect();
    if parts.len() < 2 {
        return (String::new(), None);
    }
    let last = parts[parts.len() - 1].clone();
    let before = if parts.len() >= 3 { Some(parts[parts.len() - 2].clone()) } else { None };
    (last, before)
}

// ── the rules ────────────────────────────────────────────────────────────────

fn danger(code: &'static str, title: String, detail: String) -> Warning {
    Warning { level: "danger".into(), code: code.into(), title, detail }
}
fn caution(code: &'static str, title: String, detail: String) -> Warning {
    Warning { level: "caution".into(), code: code.into(), title, detail }
}

/// Codes that stay when the person has said the sender is fine: these are about this one email, not who sent it.
const KEPT_WHEN_TRUSTED: &[&str] = &["bank_changed", "attachment_runs_code", "attachment_disguised"];

pub fn analyze(i: &Input<'_>, acct: &AccountCtx, ctx: &MessageCtx) -> Safety {
    let mut w: Vec<Warning> = Vec::new();
    let from_email = i.from.email.trim().to_lowercase();
    let from_domain = domain_of(&from_email);
    let from_reg = registrable(&from_domain);
    let own_mail = acct.own_domains.contains(&from_reg);
    let name = i.from.name.clone().unwrap_or_default();
    let pay = payment_context(i.subject, i.text);
    let login = login_context(i.subject, i.text);
    let free = is_freemail(&from_domain);
    let asks = asks_for_payment(i.subject, i.text);

    // An address written into the name that is not the address it came from.
    static ADDR: OnceLock<Regex> = OnceLock::new();
    // Same domain (support@ for help@) is one organisation; platforms that send for a person (calendar invitations, survey
    // tools) are bulk or machine mail and show the person's address by design.
    if let Some(m) = re(&ADDR, r"(?i)[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}").find(&name) {
        let shown = m.as_str().to_lowercase();
        let shown_reg = registrable(&domain_of(&shown));
        let local = from_email.split('@').next().unwrap_or("");
        let notifier = ["notif", "calendar", "invite", "noreply", "no-reply"].iter().any(|n| local.contains(n));
        if shown != from_email && shown_reg != from_reg && !own_mail && !i.bulk && !notifier {
            let lvl = if acct.known_domains.contains(&shown_reg) || pay || login { danger } else { caution };
            w.push(lvl(
                "name_hides_address",
                format!("The name shows {} but it came from {}", m.as_str(), from_email),
                "The address in the sender's name is not the one this email was sent from — a common trick to look like someone else.".into(),
            ));
        }
    }

    // Look-alike letters in the subject or the name.
    if !own_mail && (disguised_text(i.subject) || disguised_text(&name)) {
        w.push(danger(
            "disguised_text",
            "Written with look-alike letters".into(),
            "Some letters in the subject or sender's name come from other alphabets so they look normal but slip past spam filters. Real companies don't do this.".into(),
        ));
    }

    // A brand's name from outside the brand's domains.
    if !from_domain.is_empty() && !own_mail {
        let nw = words(&name).join(" ");
        for (brand, shown, domains) in BRANDS {
            let hit = format!(" {nw} ").contains(&format!(" {brand} "));
            if hit && !domains.iter().any(|d| under(&from_domain, d)) {
                // The sender's own domain, proven by DMARC: one carrying the brand's name (microsoftrewards.com) is the
                // brand's; any other (a courier delivering "your Amazon order") only says the name, so it is a caution.
                let authed = i.auth.and_then(|a| a.dmarc.as_deref()) == Some("pass");
                let label = from_reg.split('.').next().unwrap_or("");
                if authed && label.contains(&brand.replace(' ', "")) {
                    break;
                }
                let lvl = if authed && !(pay || login) { caution } else { danger };
                w.push(lvl(
                    "brand_wrong_domain",
                    format!("Says it's from {shown} but was sent from {from_domain}"),
                    format!("{shown}'s email comes from {}. Don't click its links or open attachments — go to {shown} directly if you need to check.", domains.join(" or ")),
                ));
                break;
            }
        }
    }

    // A company's name on an address that has nothing to do with it.
    if !from_domain.is_empty() && !own_mail && !w.iter().any(|x| x.code == "brand_wrong_domain") {
        let ws = words(&name);
        let org = ws.iter().any(|x| ORG_MARKERS.contains(&x.as_str()));
        let role = ws.iter().any(|x| ROLE_WORDS.contains(&x.as_str()));
        let significant: Vec<&String> = ws.iter().filter(|x| x.chars().count() >= 3 && !FILLER.contains(&x.as_str())).collect();
        let dom_flat: String = from_domain.chars().filter(|c| c.is_alphanumeric()).collect();
        let initials: String = significant.iter().filter_map(|x| x.chars().next()).collect();
        let matches = significant.iter().any(|x| {
            let stem: String = x.chars().take(4).collect();
            dom_flat.contains(x.as_str()) || (stem.chars().count() >= 4 && dom_flat.contains(&stem))
        }) || (initials.chars().count() >= 2 && dom_flat.contains(&initials));
        if free && (org || role) {
            let lvl = if pay { danger } else { caution };
            w.push(lvl(
                "company_on_freemail",
                format!("\u{201c}{}\u{201d} sent from a free {} address", name.trim(), from_domain),
                "Companies send from their own domain. Anyone can make a free webmail address with any name on it.".into(),
            ));
        } else if org && !significant.is_empty() && !matches && !free && !i.bulk {
            w.push(caution(
                "company_wrong_domain",
                format!("The name says \u{201c}{}\u{201d} but the address is @{}", name.trim(), from_domain),
                "The company in the name doesn't appear in the sender's domain. It may be a mailing service — or someone using the company's name.".into(),
            ));
        }
    }

    // Someone this account writes to, by name, from a different address.
    if !own_mail && !ctx.sender_known && !i.bulk {
        let key = words(&name).join(" ");
        if key.split(' ').count() >= 2 {
            if let Some(known) = acct.known_names.get(&key) {
                if !known.iter().any(|e| *e == from_email || registrable(&domain_of(e)) == from_reg) {
                    let lvl = if pay || free { danger } else { caution };
                    w.push(lvl(
                        "known_name_new_address",
                        format!("{} usually writes from {}", name.trim(), known[0]),
                        format!("This came from {from_email}, an address you haven't had mail from before. If it asks for anything, check with them another way."),
                    ));
                }
            }
        }
    }

    // A domain dressed up as one this account deals with.
    // (Not for an address that has written before: the known "look-alike" is then more likely a typo of ours.)
    if !from_reg.is_empty() && !acct.known_domains.contains(&from_reg) && !free && !ctx.sender_known {
        if let Some(k) = lookalike_of(&from_domain, acct.known_domains.iter()) {
            w.push(danger(
                "lookalike_domain",
                format!("{from_reg} looks like {k}, but isn't"),
                format!("You deal with {k}. This sender's domain is made to look the same — a classic trick for fake invoices and password requests."),
            ));
        } else if exotic_domain(&from_domain) {
            w.push(caution(
                "exotic_domain",
                format!("The sender's domain uses unusual letters ({from_domain})"),
                "Letters from other alphabets can make a domain look like a familiar one.".into(),
            ));
        }
    }

    // The receiving server's own verdict.
    if let Some(a) = i.auth {
        let is = |v: &Option<String>, x: &[&str]| v.as_deref().is_some_and(|s| x.contains(&s));
        if is(&a.dmarc, &["fail"]) {
            // Mailing lists and website forms fail DMARC all the time; on its own it is a caution. Asking for money or a
            // login, or with another warning (raised to danger at the end), it is a danger.
            let lvl = if asks || login { danger } else { caution };
            w.push(lvl(
                "dmarc_fail",
                format!("Failed the check that it really came from {from_domain}"),
                "Your mail server checked this email against the sender's published rules (DMARC) and it failed: it was probably not sent by them.".into(),
            ));
        } else if a.dmarc.as_deref() != Some("pass") && is(&a.spf, &["fail", "softfail"]) && !is(&a.dkim, &["pass"]) {
            w.push(caution(
                "spf_fail",
                format!("{from_domain} doesn't list the server that sent this"),
                "Your mail server's sender checks (SPF/DKIM) did not pass. Forwarded mail can do this, but so does spoofing.".into(),
            ));
        }
    }

    // Replies that go elsewhere.
    for r in i.reply_to {
        let re_email = r.email.trim().to_lowercase();
        let rd = domain_of(&re_email);
        let rr = registrable(&rd);
        if rr.is_empty() || rr == from_reg || acct.own_domains.contains(&rr) || acct.known_domains.contains(&rr) {
            continue;
        }
        if is_freemail(&rd) && !free {
            // Website contact forms set Reply-To to the visitor; that is normal. With a request for money or a login it is
            // the classic fraud.
            let lvl = if asks || login { danger } else { caution };
            w.push(lvl(
                "reply_to_freemail",
                format!("Replies go to {re_email}, not {from_domain}"),
                "The sender's address is a company's, but pressing Reply would send your answer to a free webmail account.".into(),
            ));
        } else if !i.bulk && (asks || login) {
            w.push(caution(
                "reply_to_elsewhere",
                format!("Replies go to {re_email}, not {from_domain}"),
                "Check the address before you send anything back.".into(),
            ));
        }
        break;
    }

    // Links.
    if let Some(html) = i.html {
        let mut seen_mismatch = false;
        let mut seen_ip = false;
        let mut seen_short = false;
        for (shown, href) in anchors(html) {
            if own_mail {
                break;
            }
            let Ok(u) = url::Url::parse(&href) else { continue };
            let u = real_target(u);
            if !matches!(u.scheme(), "http" | "https") {
                continue;
            }
            let Some(host) = u.host_str().map(|h| h.to_lowercase()) else { continue };
            let tracked = TRACKERS.iter().any(|t| under(&host, t));
            if !seen_mismatch && !i.bulk && !tracked {
                if let Some(sh) = shown_host(&shown) {
                    // "gov.uk" shown, "www.gov.uk" opened: one is under the other, the same site.
                    let related = under(&host, &registrable(&sh)) || under(&sh, &registrable(&host));
                    // A link to the sender's own domain, the sender proven by DMARC (a shop's name shown, its platform opened).
                    let own_site = registrable(&host) == from_reg && i.auth.and_then(|a| a.dmarc.as_deref()) == Some("pass");
                    if !related && !own_site {
                        seen_mismatch = true;
                        w.push(danger(
                            "link_mismatch",
                            format!("A link shows {sh} but goes to {}", registrable(&host)),
                            "The words of the link name one site and the link opens another. Don't use it.".into(),
                        ));
                    }
                }
            }
            if !seen_ip && matches!(u.host(), Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_))) {
                seen_ip = true;
                w.push(caution("link_ip", format!("A link goes to a bare internet address ({host})"), "Real companies link to their own named sites.".into()));
            }
            if !seen_short && !i.bulk && !ctx.sender_known && (asks || login) && SHORTENERS.contains(&host.as_str()) {
                seen_short = true;
                w.push(caution("link_short", format!("A shortened link ({host}) hides where it goes"), "In an email about a login or a payment, open the company's site yourself instead.".into()));
            }
        }
    }

    // Bank details that are not the ones this domain used before.
    // Newsletters and notifications are not invoices: their numbers are not bank details anyone is asked to pay.
    let details = if i.bulk { Vec::new() } else { bank_details(i.text) };
    if !details.is_empty() && !own_mail {
        let earlier: HashSet<&str> = ctx.earlier_bank.iter().map(|(d, _)| d.as_str()).collect();
        // Compare like with like: a sort code with earlier sort codes, an IBAN with earlier IBANs.
        let changed: Vec<&String> = details
            .iter()
            .filter(|d| {
                let kind = d.split(':').next().unwrap_or("");
                ctx.earlier_bank.iter().any(|(e, _)| e.starts_with(&format!("{kind}:"))) && !earlier.contains(d.as_str())
            })
            .collect();
        if let Some(d) = changed.first() {
            let kind = d.split(':').next().unwrap_or("");
            let before = ctx.earlier_bank.iter().filter(|(e, _)| e.starts_with(&format!("{kind}:"))).max_by_key(|(_, t)| *t);
            w.push(danger(
                "bank_changed",
                format!("Different bank details from earlier {from_reg} emails"),
                format!(
                    "This email gives {}{}. Before paying, phone them on a number you already have — not one in this email — and confirm the change.",
                    bank_label(d),
                    before.map(|(b, _)| format!("; earlier emails from {from_reg} gave {}", bank_label(b))).unwrap_or_default()
                ),
            ));
        } else if says_bank_changed(i.text) && ctx.earlier_bank.is_empty() {
            w.push(caution(
                "bank_change_claimed",
                "Says the bank details have changed".into(),
                "Fraudsters send exactly this. Confirm by phone on a number you already have before paying.".into(),
            ));
        }
    } else if says_bank_changed(i.text) && !own_mail && !i.bulk {
        w.push(caution(
            "bank_change_claimed",
            "Says the bank details have changed".into(),
            "Fraudsters send exactly this. Confirm by phone on a number you already have before paying.".into(),
        ));
    }

    // A stranger's first email asking for money.
    if asks && !ctx.sender_known && !i.bulk && !own_mail && !w.iter().any(|x| x.level == "danger") {
        w.push(caution(
            "first_contact_payment",
            format!("First email from {from_email}, and it asks for payment"),
            "You haven't had mail from this sender before. Make sure you know who they are before paying anything.".into(),
        ));
    }

    // Attachments.
    for a in i.attachments.iter().filter(|a| !a.inline) {
        let (ext, before) = ext_of(&a.filename);
        if RUNS_CODE.contains(&ext.as_str()) {
            if let Some(b) = before.as_deref().filter(|b| DOC_LIKE.contains(b)) {
                w.push(danger(
                    "attachment_disguised",
                    format!("\u{201c}{}\u{201d} pretends to be a .{b} file", a.filename),
                    format!("It is really a .{ext} file, which can run code on your computer. Don't open it."),
                ));
            } else {
                let html_like = matches!(ext.as_str(), "html" | "htm" | "shtml" | "xhtml" | "svg");
                let lvl = if html_like && !(pay || login) { caution } else { danger };
                w.push(lvl(
                    "attachment_runs_code",
                    format!("\u{201c}{}\u{201d} can run code when opened", a.filename),
                    if html_like {
                        "Web-page attachments are often fake login pages. Don't type a password into one.".into()
                    } else {
                        "Files like this can install malware. Don't open it unless you were expecting it.".into()
                    },
                ));
            }
            break;
        } else if MACRO_DOCS.contains(&ext.as_str()) {
            w.push(caution(
                "attachment_macros",
                format!("\u{201c}{}\u{201d} can contain macros", a.filename),
                "Don't press \u{201c}Enable content\u{201d} if it asks.".into(),
            ));
            break;
        }
    }

    // A failed DMARC check with another sign that someone is being impersonated is a fake. (Not a form's Reply-To or a
    // first email: those are normal for the website forms and lists that fail DMARC.)
    let other = w.iter().any(|x| {
        matches!(x.code.as_str(), "brand_wrong_domain" | "company_wrong_domain" | "company_on_freemail" | "known_name_new_address"
            | "name_hides_address" | "link_mismatch" | "disguised_text" | "lookalike_domain")
    });
    if other {
        for x in w.iter_mut().filter(|x| x.code == "dmarc_fail") {
            x.level = "danger".into();
        }
    }
    if ctx.trusted {
        w.retain(|x| KEPT_WHEN_TRUSTED.contains(&x.code.as_str()));
    }
    w.sort_by_key(|x| if x.level == "danger" { 0 } else { 1 });
    let level = if w.iter().any(|x| x.level == "danger") { LEVEL_DANGER } else if w.is_empty() { LEVEL_NONE } else { LEVEL_CAUTION };
    Safety { level, warnings: w, trusted: ctx.trusted, body_checked: i.has_body }
}

// ── the store ────────────────────────────────────────────────────────────────

pub fn account_ctx(conn: &Connection, account_id: &str) -> Result<AccountCtx> {
    let mut own = HashSet::new();
    let mut st = conn.prepare("SELECT email FROM accounts UNION SELECT email FROM identities")?;
    for e in st.query_map([], |r| r.get::<_, String>(0))? {
        let d = registrable(&domain_of(&e?));
        if !d.is_empty() && !is_freemail(&d) {
            own.insert(d);
        }
    }
    let mut known: HashSet<String> = own.clone();
    let mut st = conn.prepare("SELECT email FROM correspondents WHERE account_id = ?1")?;
    for e in st.query_map(params![account_id], |r| r.get::<_, String>(0))? {
        let d = registrable(&domain_of(&e?));
        if !d.is_empty() {
            known.insert(d);
        }
    }
    let mut names: HashMap<String, Vec<String>> = HashMap::new();
    let mut st = conn.prepare("SELECT name, email FROM contacts_seen WHERE sent_to > 0 AND name IS NOT NULL AND name <> ''")?;
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let (n, e) = row?;
        let key = words(&n).join(" ");
        if key.split(' ').count() >= 2 {
            names.entry(key).or_default().push(e.to_lowercase());
        }
    }
    Ok(AccountCtx { own_domains: own, known_domains: known, known_names: names })
}

pub fn message_ctx(conn: &Connection, account_id: &str, rowid: i64, from_email: &str, date: i64) -> Result<MessageCtx> {
    let from = from_email.trim().to_lowercase();
    let wrote_to: bool = conn
        .query_row("SELECT 1 FROM correspondents WHERE account_id = ?1 AND email = ?2", params![account_id, from], |_| Ok(true))
        .optional()?
        .unwrap_or(false);
    let earlier: bool = wrote_to
        || conn
            .query_row(
                // The address as stored, so messages_from serves it. "+account_id" keeps SQLite off the account index,
                // which it otherwise picks and then reads every message of the account for each one checked (measured on
                // a 133k-message mailbox: ~120 messages a second, against the from index's per-sender handful).
                "SELECT 1 FROM messages WHERE from_email = ?2 AND +account_id = ?1 AND date < ?3 AND id <> ?4 LIMIT 1",
                params![account_id, from_email.trim(), date, rowid],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
    let trusted: bool = conn
        .query_row("SELECT 1 FROM safe_senders WHERE account_id = ?1 AND email = ?2", params![account_id, from], |_| Ok(true))
        .optional()?
        .unwrap_or(false);
    let dom = registrable(&domain_of(&from));
    let mut st = conn.prepare("SELECT detail, first_date FROM bank_seen WHERE account_id = ?1 AND domain = ?2 AND message_rowid <> ?3 AND first_date <= ?4")?;
    let earlier_bank = st
        .query_map(params![account_id, dom, rowid, date], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(MessageCtx { sender_known: earlier, trusted, earlier_bank })
}

/// The stored facts of one message the checks read.
struct Row {
    id: i64,
    account_id: String,
    from: Address,
    reply_to: Vec<Address>,
    auth: Option<AuthResults>,
    bulk: bool,
    subject: String,
    date: i64,
}

fn read_row(conn: &Connection, id: i64) -> Result<Row> {
    Ok(conn.query_row(
        "SELECT id, account_id, from_name, from_email, reply_to_json, auth_json, bulk, subject, date FROM messages WHERE id = ?1",
        params![id],
        |r| {
            let rt: String = r.get(4)?;
            let auth: Option<String> = r.get(5)?;
            Ok(Row {
                id: r.get(0)?,
                account_id: r.get(1)?,
                from: Address { name: r.get(2)?, email: r.get(3)? },
                reply_to: serde_json::from_str(&rt).unwrap_or_default(),
                auth: auth.and_then(|s| serde_json::from_str(&s).ok()),
                bulk: r.get(6)?,
                subject: r.get(7)?,
                date: r.get(8)?,
            })
        },
    )?)
}

fn run(row: &Row, raw: Option<&[u8]>, acct: &AccountCtx, ctx: &MessageCtx) -> (Safety, Vec<String>) {
    let parsed = raw.map(|b| mime::parse_body(b, false));
    let text = parsed.as_ref().map(|p| p.index_text.as_str()).unwrap_or("");
    let html = parsed.as_ref().and_then(|p| if p.plain_text { None } else { p.html.as_deref() });
    let empty: Vec<Attachment> = Vec::new();
    let atts = parsed.as_ref().map(|p| &p.attachments).unwrap_or(&empty);
    let input = Input {
        from: &row.from,
        reply_to: &row.reply_to,
        auth: row.auth.as_ref(),
        bulk: row.bulk,
        subject: &row.subject,
        text,
        html,
        attachments: atts,
        has_body: raw.is_some(),
    };
    let s = analyze(&input, acct, ctx);
    let details = bank_details(text);
    (s, details)
}

/// The reading pane's verdict for one message, worked out now (a sender marked fine a moment ago counts at once).
pub fn assess(conn: &Connection, id: i64, raw: &[u8]) -> Result<Safety> {
    let row = read_row(conn, id)?;
    let acct = account_ctx(conn, &row.account_id)?;
    let ctx = message_ctx(conn, &row.account_id, row.id, &row.from.email, row.date)?;
    Ok(run(&row, if raw.is_empty() { None } else { Some(raw) }, &acct, &ctx).0)
}

/// One verdict worked out by the scanner, written by `store_verdicts`.
pub struct Verdict {
    pub id: i64,
    pub account_id: String,
    pub level: i64,
    pub checked: i64,
    pub domain: String,
    pub date: i64,
    /// Bank details to remember for the domain: only from mail that raised no danger, so a fraudster's details never
    /// become the ones a later email is compared against.
    pub bank: Vec<String>,
}

/// Messages not checked yet (or checked before their body arrived), oldest first so earlier bank details are on record
/// before later mail is compared with them. Works on the read connection; nothing is written.
pub fn scan_batch(conn: &Connection, limit: i64) -> Result<Vec<Verdict>> {
    let mut st = conn.prepare(
        "SELECT id, body_cached FROM messages WHERE risk_checked = 0 OR (risk_checked = 1 AND body_cached = 1) ORDER BY date ASC, id ASC LIMIT ?1",
    )?;
    let ids: Vec<(i64, bool)> = st.query_map(params![limit], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut accts: HashMap<String, AccountCtx> = HashMap::new();
    // Bank details found earlier in this same batch (not written yet): (account, domain) → [(detail, date)].
    let mut batch_bank: HashMap<(String, String), Vec<(String, i64)>> = HashMap::new();
    let mut out = Vec::new();
    for (id, cached) in ids {
        let row = read_row(conn, id)?;
        if !accts.contains_key(&row.account_id) {
            accts.insert(row.account_id.clone(), account_ctx(conn, &row.account_id)?);
        }
        let acct = &accts[&row.account_id];
        let mut ctx = message_ctx(conn, &row.account_id, row.id, &row.from.email, row.date)?;
        let domain = registrable(&domain_of(&row.from.email));
        if let Some(more) = batch_bank.get(&(row.account_id.clone(), domain.clone())) {
            ctx.earlier_bank.extend(more.iter().cloned());
        }
        let raw: Option<Vec<u8>> = if cached {
            conn.query_row("SELECT raw FROM bodies WHERE message_rowid = ?1", params![id], |r| r.get(0)).optional()?
        } else {
            None
        };
        let (s, details) = run(&row, raw.as_deref(), acct, &ctx);
        let bank = if s.level < LEVEL_DANGER && !acct.own_domains.contains(&domain) { details } else { Vec::new() };
        if !bank.is_empty() {
            let e = batch_bank.entry((row.account_id.clone(), domain.clone())).or_default();
            for d in &bank {
                e.push((d.clone(), row.date));
            }
        }
        out.push(Verdict { id, account_id: row.account_id, level: s.level, checked: if raw.is_some() { 2 } else { 1 }, domain, date: row.date, bank });
    }
    Ok(out)
}

/// Writes the scanner's verdicts. → the folders whose lists changed (a message now marked, or no longer).
pub fn store_verdicts(conn: &mut Connection, vs: &[Verdict]) -> Result<Vec<String>> {
    let tx = conn.transaction()?;
    let mut folders: Vec<String> = Vec::new();
    for v in vs {
        let before: Option<(i64, i64)> = tx
            .query_row("SELECT risk, folder_id FROM messages WHERE id = ?1", params![v.id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((old, folder)) = before else { continue };
        tx.execute("UPDATE messages SET risk = ?2, risk_checked = ?3 WHERE id = ?1", params![v.id, v.level, v.checked])?;
        for d in &v.bank {
            tx.execute(
                "INSERT OR IGNORE INTO bank_seen(account_id, domain, detail, first_date, message_rowid) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![v.account_id, v.domain, d, v.date, v.id],
            )?;
        }
        if old != v.level {
            let f = folder.to_string();
            if !folders.contains(&f) {
                folders.push(f);
            }
        }
    }
    tx.commit()?;
    Ok(folders)
}

/// "This sender is fine" (or not, `trusted` false): remembered for the account, and the sender's mail checked again.
pub fn set_trusted(conn: &Connection, message_id: i64, trusted: bool) -> Result<(String, String)> {
    let (account, from): (String, String) =
        conn.query_row("SELECT account_id, lower(from_email) FROM messages WHERE id = ?1", params![message_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if trusted {
        conn.execute("INSERT OR IGNORE INTO safe_senders(account_id, email) VALUES (?1, ?2)", params![account, from])?;
    } else {
        conn.execute("DELETE FROM safe_senders WHERE account_id = ?1 AND email = ?2", params![account, from])?;
    }
    conn.execute("UPDATE messages SET risk_checked = 0 WHERE account_id = ?1 AND lower(from_email) = ?2", params![account, from])?;
    Ok((account, from))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(name: &str, email: &str) -> Address {
        Address { name: if name.is_empty() { None } else { Some(name.into()) }, email: email.into() }
    }

    fn acct() -> AccountCtx {
        let mut a = AccountCtx::default();
        a.own_domains.insert("wolf.uk.com".into());
        for d in ["wolf.uk.com", "acmesupplies.co.uk", "gmail.com"] {
            a.known_domains.insert(d.into());
        }
        a.known_names.insert("ian clevett".into(), vec!["ian@wolf.uk.com".into()]);
        a
    }

    fn check(from: Address, subject: &str, text: &str, html: Option<&str>, ctx: &MessageCtx) -> Safety {
        let input = Input { from: &from, reply_to: &[], auth: None, bulk: false, subject, text, html, attachments: &[], has_body: true };
        analyze(&input, &acct(), ctx)
    }

    fn codes(s: &Safety) -> Vec<&str> {
        s.warnings.iter().map(|w| w.code.as_str()).collect()
    }

    #[test]
    fn registrable_domains() {
        assert_eq!(registrable("mail.acme.co.uk"), "acme.co.uk");
        assert_eq!(registrable("wolf.uk.com"), "wolf.uk.com");
        assert_eq!(registrable("a.b.example.com"), "example.com");
        assert_eq!(registrable("tax.service.gov.uk"), "service.gov.uk");
    }

    #[test]
    fn brand_from_elsewhere_is_danger_and_from_its_domain_is_not() {
        let known = MessageCtx { sender_known: true, ..Default::default() };
        let s = check(addr("PayPal Service", "service@paypal-secure-alerts.com"), "Your account", "", None, &known);
        assert_eq!(s.level, LEVEL_DANGER);
        assert!(codes(&s).contains(&"brand_wrong_domain"));
        let ok = check(addr("PayPal", "service@mail.paypal.co.uk"), "Receipt", "", None, &known);
        assert!(!codes(&ok).contains(&"brand_wrong_domain"));
        let hmrc = check(addr("HMRC", "noreply@tax.service.gov.uk"), "Tax", "", None, &known);
        assert!(!codes(&hmrc).contains(&"brand_wrong_domain"));
        // "Apple" as a whole word only: "Pineapple Design" is not Apple.
        let pine = check(addr("Pineapple Design", "hello@pineappledesign.co.uk"), "Hi", "", None, &known);
        assert!(!codes(&pine).contains(&"brand_wrong_domain"));
    }

    #[test]
    fn company_name_rules() {
        let known = MessageCtx { sender_known: true, ..Default::default() };
        let free = check(addr("Acme Supplies Ltd Accounts", "acme.accounts@gmail.com"), "Invoice 4411", "Please pay the invoice", None, &known);
        assert!(codes(&free).contains(&"company_on_freemail"));
        assert_eq!(free.level, LEVEL_DANGER, "a company on webmail asking for payment");
        let other = check(addr("Northwind Traders Ltd", "billing@mailer-xyz.net"), "Hello", "", None, &known);
        assert!(codes(&other).contains(&"company_wrong_domain"));
        let fine = check(addr("Northwind Traders Ltd", "billing@northwindtraders.com"), "Hello", "", None, &known);
        assert!(!codes(&fine).contains(&"company_wrong_domain"));
        let initials = check(addr("Wolf Software Ltd", "billing@ws-mail.co.uk"), "Hello", "", None, &known);
        assert!(!codes(&initials).contains(&"company_wrong_domain"), "initials in the domain count");
        let person = check(addr("Jane Smith", "jane.smith@gmail.com"), "Hello", "", None, &known);
        assert!(person.warnings.is_empty(), "a person on webmail is normal: {:?}", person.warnings);
    }

    #[test]
    fn address_in_the_name() {
        let s = check(addr("ian@wolf.uk.com", "x@evil.example"), "Hi", "", None, &MessageCtx { sender_known: true, ..Default::default() });
        assert!(codes(&s).contains(&"name_hides_address"));
    }

    #[test]
    fn lookalikes() {
        let k: Vec<String> = vec!["wolf.uk.com".into(), "acmesupplies.co.uk".into()];
        assert_eq!(lookalike_of("wo1f.uk.com", k.iter()).as_deref(), Some("wolf.uk.com"));
        assert_eq!(lookalike_of("wolf-uk.com", k.iter()).as_deref(), Some("wolf.uk.com"));
        assert_eq!(lookalike_of("acmesuppiles.co.uk", k.iter()).as_deref(), Some("acmesupplies.co.uk"));
        assert_eq!(lookalike_of("acrnesupplies.co.uk", k.iter()).as_deref(), Some("acmesupplies.co.uk"));
        assert_eq!(lookalike_of("wоlf.uk.com", k.iter()).as_deref(), Some("wolf.uk.com"), "Cyrillic o");
        assert_eq!(lookalike_of("golf.uk.com", k.iter()), None, "a different word is not a lookalike");
        assert_eq!(lookalike_of("wolf.uk.com", k.iter()), None);
        let s = check(addr("Accounts", "accounts@wo1f.uk.com"), "Invoice", "", None, &MessageCtx::default());
        assert!(codes(&s).contains(&"lookalike_domain"));
    }

    #[test]
    fn known_name_from_new_address() {
        let s = check(addr("Ian Clevett", "ian.clevett.ceo@gmail.com"), "Quick favour", "Can you make a payment today", None, &MessageCtx::default());
        assert!(codes(&s).contains(&"known_name_new_address"));
        assert_eq!(s.level, LEVEL_DANGER);
    }

    #[test]
    fn auth_and_reply_to() {
        let from = addr("Acme", "news@acmesupplies.co.uk");
        let auth = AuthResults { spf: Some("pass".into()), dkim: Some("pass".into()), dmarc: Some("fail".into()) };
        let rt = [addr("", "acme.payments@outlook.com")];
        let input = Input { from: &from, reply_to: &rt, auth: Some(&auth), bulk: false, subject: "Hi", text: "", html: None, attachments: &[], has_body: true };
        let s = analyze(&input, &acct(), &MessageCtx { sender_known: true, ..Default::default() });
        assert!(codes(&s).contains(&"dmarc_fail"));
        assert!(codes(&s).contains(&"reply_to_freemail"));
    }

    #[test]
    fn links() {
        let html = r#"<p>Log in at <a href="https://evil.example/hsbc">www.hsbc.co.uk</a> or <a href="http://192.0.2.7/x">here</a>.</p>
                      <a href="https://www.hsbc.co.uk/login">https://www.hsbc.co.uk</a>"#;
        let s = check(addr("Jane", "jane@acmesupplies.co.uk"), "Log in", "log in please", Some(html), &MessageCtx { sender_known: true, ..Default::default() });
        assert!(codes(&s).contains(&"link_mismatch"));
        assert!(codes(&s).contains(&"link_ip"));
        assert_eq!(s.warnings.iter().filter(|w| w.code == "link_mismatch").count(), 1);
    }

    #[test]
    fn iban_check_digits() {
        // ISO 13616 example IBAN (GB) and the same with one digit changed.
        assert!(iban_valid("GB82 WEST 1234 5698 7654 32"));
        assert!(!iban_valid("GB82 WEST 1234 5698 7654 33"));
        assert_eq!(bank_details("IBAN GB82 WEST 1234 5698 7654 32 THANKS"), vec!["IBAN:GB82WEST12345698765432"]);
    }

    #[test]
    fn bank_details_and_change() {
        let t = "Please pay to sort code 12-34-56, account number 87654321. IBAN GB82 WEST 1234 5698 7654 32";
        assert_eq!(bank_details(t), vec!["IBAN:GB82WEST12345698765432", "SORT:123456", "ACCT:87654321"]);
        let earlier = MessageCtx { sender_known: true, trusted: true, earlier_bank: vec![("SORT:654321".into(), 1), ("ACCT:11112222".into(), 1)] };
        let s = check(addr("Acme", "accounts@acmesupplies.co.uk"), "Invoice 12", t, None, &earlier);
        assert!(codes(&s).contains(&"bank_changed"), "kept even for a trusted sender");
        assert_eq!(s.level, LEVEL_DANGER);
        let same = MessageCtx { sender_known: true, earlier_bank: vec![("SORT:123456".into(), 1), ("ACCT:87654321".into(), 1)], ..Default::default() };
        let s2 = check(addr("Acme", "accounts@acmesupplies.co.uk"), "Invoice 12", "Sort code 12-34-56 account number 87654321", None, &same);
        assert!(!codes(&s2).contains(&"bank_changed"));
        let claim = check(addr("Acme", "accounts@acmesupplies.co.uk"), "Update", "Please note our bank details have changed", None, &MessageCtx { sender_known: true, ..Default::default() });
        assert!(codes(&claim).contains(&"bank_change_claimed"));
    }

    #[test]
    fn first_contact_payment() {
        let s = check(addr("Bob", "bob@newco.example"), "Invoice overdue", "The invoice is overdue", None, &MessageCtx::default());
        assert!(codes(&s).contains(&"first_contact_payment"));
        let k = check(addr("Bob", "bob@newco.example"), "Invoice overdue", "", None, &MessageCtx { sender_known: true, ..Default::default() });
        assert!(!codes(&k).contains(&"first_contact_payment"));
    }

    #[test]
    fn attachments() {
        let mk = |n: &str| Attachment { id: "1".into(), filename: n.into(), mime: "application/octet-stream".into(), size: 1, inline: false, content_id: None };
        let from = addr("Jane", "jane@acmesupplies.co.uk");
        let ctx = MessageCtx { sender_known: true, ..Default::default() };
        let run = |a: Vec<Attachment>, subj: &str| {
            let input = Input { from: &from, reply_to: &[], auth: None, bulk: false, subject: subj, text: "", html: None, attachments: &a, has_body: true };
            analyze(&input, &acct(), &ctx)
        };
        assert!(codes(&run(vec![mk("invoice.pdf.exe")], "Hi")).contains(&"attachment_disguised"));
        let h = run(vec![mk("Remittance.html")], "Remittance advice");
        assert_eq!(h.level, LEVEL_DANGER);
        assert_eq!(run(vec![mk("menu.html")], "Lunch").level, LEVEL_CAUTION);
        assert!(codes(&run(vec![mk("budget.xlsm")], "Hi")).contains(&"attachment_macros"));
        assert!(run(vec![mk("report.pdf")], "Hi").warnings.is_empty());
    }

    #[test]
    fn trusted_sender_keeps_only_message_warnings() {
        let s = check(addr("Northwind Traders Ltd", "billing@mailer-xyz.net"), "Hello", "", None, &MessageCtx { sender_known: true, trusted: true, ..Default::default() });
        assert!(s.warnings.is_empty());
        assert!(s.trusted);
    }

    /// Through the store: the scanner checks stored mail oldest first, remembers a supplier's bank details from a
    /// clean invoice, marks a later invoice with different ones, lists it under Suspicious, and a sender marked fine
    /// keeps that warning (it is about the email) while losing the others.
    #[test]
    fn scanner_end_to_end() {
        use crate::db::Store;
        use crate::model::{SortOrder, UnifiedView, ViewQuery};
        let s = Store::open_in_memory().unwrap();
        s.with(|c| {
            c.execute_batch(
                "INSERT INTO accounts(id,name,email,color,provider,imap_json,smtp_json) VALUES ('a','A','paul@wolf.uk.com','#c41e1e','imap','{}','{}');
                 INSERT INTO folders(id,account_id,path,name,role) VALUES (1,'a','INBOX','Inbox','inbox');",
            )?;
            let put = |id: i64, from: &str, date: i64, body: &str| -> Result<()> {
                c.execute(
                    "INSERT INTO messages(id,account_id,folder_id,uid,thread_id,subject,from_name,from_email,date,body_cached) VALUES (?1,'a',1,?1,?2,'Invoice','Acme Supplies',?3,?4,1)",
                    params![id, format!("t{id}"), from, date],
                )?;
                let raw = format!("From: Acme Supplies <{from}>\r\nSubject: Invoice\r\nContent-Type: text/plain\r\n\r\n{body}\r\n");
                c.execute("INSERT INTO bodies(message_rowid, raw) VALUES (?1, ?2)", params![id, raw.as_bytes()])?;
                Ok(())
            };
            put(1, "accounts@acmesupplies.co.uk", 1_000, "Invoice 1. Pay to sort code 12-34-56, account number 87654321.")?;
            put(2, "accounts@acmesupplies.co.uk", 2_000, "Invoice 2. Our bank details have changed: sort code 65-43-21, account number 11112222.")?;
            put(3, "accounts@acmesupplies.co.uk", 3_000, "Invoice 3. Pay to sort code 12-34-56, account number 87654321.")?;
            Ok(())
        })
        .unwrap();
        let v = s.with(|c| scan_batch(c, 50)).unwrap();
        assert_eq!(v.iter().map(|x| (x.id, x.level)).collect::<Vec<_>>(), vec![(1, LEVEL_CAUTION), (2, LEVEL_DANGER), (3, LEVEL_NONE)]);
        // 1: a first email asking for payment (caution) — its bank details are still remembered; 3: the same details again.
        assert_eq!(v[0].bank, vec!["SORT:123456", "ACCT:87654321"]);
        assert!(v[1].bank.is_empty(), "a flagged email's bank details are never remembered");
        s.with(|c| store_verdicts(c, &v)).unwrap();
        assert!(s.with(|c| scan_batch(c, 50)).unwrap().is_empty(), "nothing left to check");
        let page = s
            .with(|c| crate::msgstore::list_view(c, &ViewQuery { folder_id: None, unified: Some(UnifiedView::Suspicious), filter: None, search: None, threaded: false, sort: SortOrder::DateDesc, offset: 0, limit: 10, keep_ids: vec![] }))
            .unwrap();
        assert_eq!(page.items.iter().map(|m| (m.id.as_str(), m.risk)).collect::<Vec<_>>(), vec![("2", LEVEL_DANGER), ("1", LEVEL_CAUTION)]);
        let raw2: Vec<u8> = s.with(|c| Ok(c.query_row("SELECT raw FROM bodies WHERE message_rowid=2", [], |r| r.get(0))?)).unwrap();
        let open = s.with(|c| assess(c, 2, &raw2)).unwrap();
        assert!(open.warnings.iter().any(|w| w.code == "bank_changed"), "{:?}", open.warnings);
        s.with(|c| set_trusted(c, 2, true).map(|_| ())).unwrap();
        let again = s.with(|c| scan_batch(c, 50)).unwrap();
        assert_eq!(again.len(), 3, "the sender's mail is checked again");
        assert_eq!(again[1].level, LEVEL_DANGER, "changed bank details stay flagged for a trusted sender");
        let trusted = s.with(|c| assess(c, 2, &raw2)).unwrap();
        assert!(trusted.trusted && trusted.warnings.iter().all(|w| w.code == "bank_changed"), "{:?}", trusted.warnings);
    }

    /// Tuned on a real 133k-message mailbox (2 Nov 2026): what was noise there stays quiet, what was fraud still shows.
    #[test]
    fn tuned_on_a_real_mailbox() {
        let known = MessageCtx { sender_known: true, ..Default::default() };
        let run = |from: Address, subject: &str, text: &str, html: Option<&str>, auth: Option<AuthResults>, rt: Vec<Address>, bulk: bool, ctx: &MessageCtx| {
            let input = Input { from: &from, reply_to: &rt, auth: auth.as_ref(), bulk, subject, text, html, attachments: &[], has_body: true };
            analyze(&input, &acct(), ctx)
        };
        let fail = || Some(AuthResults { spf: Some("fail".into()), dkim: Some("fail".into()), dmarc: Some("fail".into()) });
        let pass = || Some(AuthResults { spf: Some("pass".into()), dkim: Some("pass".into()), dmarc: Some("pass".into()) });
        // A mailing list failing DMARC on its own is a caution; asking for payment it is a danger.
        assert_eq!(run(addr("CANA", "admin@cana.org.uk"), "eLetter", "News", None, fail(), vec![], true, &known).level, LEVEL_CAUTION);
        assert_eq!(run(addr("CANA", "admin@cana.org.uk"), "Overdue", "The invoice is overdue", None, fail(), vec![], false, &known).level, LEVEL_DANGER);
        // A website form: Reply-To is the visitor's webmail. Normal, so only a caution.
        let form = run(addr("", "wordpress@mdflinings.com"), "Enquiry", "Hello", None, None, vec![addr("", "visitor@mail.ru")], false, &known);
        assert_eq!(form.level, LEVEL_CAUTION);
        // GitHub sending as an organisation (bulk): not a fake company.
        assert!(run(addr("IntelligentWolf Ltd", "notifications@github.com"), "Run failed", "", None, None, vec![], true, &known).warnings.is_empty());
        // Microsoft 365 Safe Links wrap every link: judged by where they really go.
        let safe = r#"<a href="https://eur01.safelinks.protection.outlook.com/?url=https%3A%2F%2Fwww.hsbc.co.uk%2Flogin&amp;data=x">www.hsbc.co.uk</a>"#;
        assert!(!codes(&run(addr("Jane", "jane@acmesupplies.co.uk"), "Hi", "", Some(safe), None, vec![], false, &known)).contains(&"link_mismatch"));
        let evil = r#"<a href="https://eur01.safelinks.protection.outlook.com/?url=https%3A%2F%2Fevil.example%2Ftv&amp;data=x">tvlicensing.co.uk</a>"#;
        assert!(codes(&run(addr("TV", "x@relay.example"), "Renew", "", Some(evil), None, vec![], false, &known)).contains(&"link_mismatch"));
        let gov = r#"<a href="https://www.gov.uk/x">gov.uk</a>"#;
        assert!(run(addr("DVLA", "dvlaapiaccess@dvla.gov.uk"), "Maintenance", "", Some(gov), None, vec![], false, &known).warnings.is_empty());
        // Look-alike letters in the subject (the TV Licensing fakes).
        assert!(disguised_text("Y\u{1D5C8}ur L\u{1D5C2}cence"));
        assert!(disguised_text("Your \u{0410}nnu\u{0430}l Licence"));
        assert!(!disguised_text("Ваш счёт") && !disguised_text("Your annual licence"), "all-Cyrillic or all-Latin words are just words");
        assert!(codes(&run(addr("TV Licensing", "x@relay.example"), "Your \u{0410}nnu\u{0430}l Licence", "", None, None, vec![], false, &known)).contains(&"disguised_text"));
        // Short names are different companies, not look-alikes.
        let mut a = acct();
        a.known_domains.insert("xerox.com".into());
        assert_eq!(lookalike_of("xero.com", a.known_domains.iter()), None);
        assert_eq!(lookalike_of("mail.com", ["gmail.com".to_string()].iter()), None);
        // A brand's own authenticated domain carrying its name; a courier delivering the brand's parcel is a caution.
        assert!(run(addr("Microsoft Rewards", "x@customeremail.microsoftrewards.com"), "Rewards", "", None, pass(), vec![], true, &known).warnings.is_empty());
        assert_eq!(run(addr("Your Amazon order", "yourorder@dpd.co.uk"), "Delivery", "", None, pass(), vec![], true, &known).level, LEVEL_CAUTION);
        assert_eq!(run(addr("Amazon", "x@amazon-refunds.example"), "Refund", "", None, None, vec![], false, &known).level, LEVEL_DANGER);
        // A receipt is not a request to pay.
        assert!(run(addr("Open-Meteo", "info@open-meteo.com"), "Your receipt", "Thanks for your payment", None, None, vec![], false, &MessageCtx::default()).warnings.is_empty());
        // A newsletter's numbers are not bank details.
        let nl = MessageCtx { sender_known: true, earlier_bank: vec![("ACCT:11112222".into(), 1)], ..Default::default() };
        assert!(run(addr("News", "support@mail.example"), "Tips", "account number 12345678", None, None, vec![], true, &nl).warnings.is_empty());
    }

    #[test]
    fn own_mail_is_not_flagged() {
        let s = check(addr("Wolf Software Ltd", "paul@wolf.uk.com"), "Invoice", "Please pay the invoice", None, &MessageCtx::default());
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    }
}
