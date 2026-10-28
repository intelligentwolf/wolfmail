//! CalDAV (RFC 4791) + CardDAV (RFC 6352) client.
//! Sources: RFC 4918 §9.1 PROPFIND + 207 multistatus, §10.4 If-Match / If-None-Match; RFC 5397
//! current-user-principal; RFC 6764 §5 ".well-known/caldav|carddav" bootstrap; RFC 4791 §6.2.1 calendar-home-set,
//! §5.2.3 supported-calendar-component-set, §7.9 calendar-multiget; RFC 6352 §7.1.1 addressbook-home-set, §8.7
//! addressbook-multiget; RFC 6578 §3.2 sync-collection REPORT (empty token = initial sync; 404 per removed member;
//! 403/409 DAV:valid-sync-token when the token expired); RFC 3744 §5.4 current-user-privilege-set (write /
//! write-content); calendarserver.org "getctag" (CalendarServer ctag extension, widely deployed); Apple
//! "calendar-color".
use super::xml::{self, El, APPLE, CALDAV, CARDDAV, CS, DAV};
use crate::error::{ApiError, ErrorKind, Result};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Method, StatusCode};

pub struct Dav<'a> {
    pub http: &'a reqwest::Client,
    pub user: String,
    pub pass: String,
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionKind {
    Calendar,
    AddressBook,
}

#[derive(Debug, Clone)]
pub struct Collection {
    pub url: String,
    pub kind: CollectionKind,
    pub name: String,
    pub color: Option<String>,
    pub ctag: Option<String>,
    pub sync_token: Option<String>,
    pub read_only: bool,
}

#[derive(Debug, Default)]
pub struct Delta {
    /// (href, etag) of members to (re)fetch
    pub changed: Vec<(String, Option<String>)>,
    pub removed: Vec<String>,
    pub token: Option<String>,
    /// true: `changed` is the complete membership (anything else local is gone)
    pub full: bool,
}

pub fn join(base: &str, href: &str) -> String {
    match url::Url::parse(base).and_then(|b| b.join(href)) {
        Ok(u) => u.to_string(),
        Err(_) => href.to_string(),
    }
}

/// Compare hrefs by path, decoding percent-escapes (servers mix encoded and raw forms).
pub fn same_href(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let p = url::Url::parse(s).map(|u| u.path().to_string()).unwrap_or_else(|_| s.to_string());
        percent_decode(&p).trim_end_matches('/').to_string()
    };
    norm(a) == norm(b)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl<'a> Dav<'a> {
    pub async fn request(&self, method: &str, url: &str, depth: Option<&str>, body: Option<String>, ctype: &str, extra: &[(&str, &str)]) -> Result<Resp> {
        let m = Method::from_bytes(method.as_bytes()).map_err(|e| ApiError::internal(e.to_string()))?;
        let mut rq = self.http.request(m, url).basic_auth(&self.user, Some(&self.pass));
        if let Some(d) = depth {
            rq = rq.header("Depth", d);
        }
        for (k, v) in extra {
            rq = rq.header(*k, HeaderValue::from_str(v).map_err(|e| ApiError::internal(e.to_string()))?);
        }
        if let Some(b) = body {
            rq = rq.header("Content-Type", ctype).body(b);
        }
        let resp = rq.send().await?;
        let status = resp.status();
        let final_url = resp.url().to_string();
        let headers = resp.headers().clone();
        let body = resp.text().await.unwrap_or_default();
        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiError::auth("The calendar/contacts server refused the user name or password."));
        }
        Ok(Resp { status, headers, body, url: final_url })
    }

    async fn propfind(&self, url: &str, depth: &str, props: &str) -> Result<Resp> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="{CALDAV}" xmlns:card="{CARDDAV}" xmlns:cs="{CS}" xmlns:a="{APPLE}"><d:prop>{props}</d:prop></d:propfind>"#
        );
        let r = self.request("PROPFIND", url, Some(depth), Some(body), "application/xml; charset=utf-8", &[]).await?;
        if r.status.as_u16() != 207 {
            return Err(ApiError::server(format!("The server answered PROPFIND with HTTP {}.", r.status.as_u16())));
        }
        Ok(r)
    }

    /// Find the principal and its calendar/address-book homes, starting from whatever URL the user typed.
    pub async fn discover(&self, start: &str) -> Result<(Vec<String>, Vec<String>)> {
        let origin = url::Url::parse(start).map_err(|_| ApiError::invalid("url", "Enter the full server address, starting with https://"))?;
        let mut candidates = vec![start.to_string()];
        for wk in ["/.well-known/caldav", "/.well-known/carddav"] {
            if let Ok(u) = origin.join(wk) {
                candidates.push(u.to_string());
            }
        }
        let mut cal_homes: Vec<String> = Vec::new();
        let mut card_homes: Vec<String> = Vec::new();
        let mut direct: Vec<String> = Vec::new();
        let mut last_err: Option<ApiError> = None;
        for cand in candidates {
            let r = match self
                .propfind(&cand, "0", "<d:current-user-principal/><d:resourcetype/><c:calendar-home-set/><card:addressbook-home-set/>")
                .await
            {
                Ok(r) => r,
                Err(e) if e.kind == ErrorKind::Auth => return Err(e),
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            };
            let Some(tree) = xml::parse(&r.body) else { continue };
            // The URL itself may be a collection (e.g. a calendar link pasted from a provider).
            if let Some(rt) = tree.find(DAV, "resourcetype") {
                if rt.child(CALDAV, "calendar").is_some() || rt.child(CARDDAV, "addressbook").is_some() {
                    direct.push(r.url.clone());
                }
            }
            collect_hrefs(&tree, CALDAV, "calendar-home-set", &r.url, &mut cal_homes);
            collect_hrefs(&tree, CARDDAV, "addressbook-home-set", &r.url, &mut card_homes);
            let mut principals = Vec::new();
            collect_hrefs(&tree, DAV, "current-user-principal", &r.url, &mut principals);
            for p in principals {
                if let Ok(pr) = self.propfind(&p, "0", "<c:calendar-home-set/><card:addressbook-home-set/>").await {
                    if let Some(t) = xml::parse(&pr.body) {
                        collect_hrefs(&t, CALDAV, "calendar-home-set", &pr.url, &mut cal_homes);
                        collect_hrefs(&t, CARDDAV, "addressbook-home-set", &pr.url, &mut card_homes);
                    }
                }
            }
            if !cal_homes.is_empty() || !card_homes.is_empty() {
                break;
            }
        }
        // A pasted collection URL counts as its own "home" (listed with depth 0 below).
        for d in direct {
            if !cal_homes.iter().chain(card_homes.iter()).any(|h| same_href(h, &d)) {
                cal_homes.push(d);
            }
        }
        if cal_homes.is_empty() && card_homes.is_empty() {
            return Err(last_err.unwrap_or_else(|| {
                ApiError::server("No calendars or address books were found at that address. Check the server address (your provider's CalDAV/CardDAV URL).")
            }));
        }
        cal_homes.dedup_by(|a, b| same_href(a, b));
        card_homes.dedup_by(|a, b| same_href(a, b));
        Ok((cal_homes, card_homes))
    }

    /// List calendars/address books in a home (depth 1), or describe the home itself if it is a collection.
    pub async fn collections(&self, home: &str) -> Result<Vec<Collection>> {
        let props = "<d:resourcetype/><d:displayname/><cs:getctag/><d:sync-token/><a:calendar-color/><c:supported-calendar-component-set/><d:current-user-privilege-set/>";
        let r = self.propfind(home, "1", props).await?;
        let tree = xml::parse(&r.body).ok_or_else(|| ApiError::server("The server sent a response WolfMail could not read."))?;
        let mut out = Vec::new();
        for resp in tree.children_of(DAV, "response") {
            let Some(href) = resp.text_of(DAV, "href") else { continue };
            let Some(prop) = ok_prop(resp) else { continue };
            let Some(rt) = prop.child(DAV, "resourcetype") else { continue };
            let kind = if rt.child(CALDAV, "calendar").is_some() {
                // Skip task-only (VTODO) collections: WolfMail shows events. No set = all components allowed.
                if let Some(set) = prop.child(CALDAV, "supported-calendar-component-set") {
                    let comps: Vec<&str> = set.children_of(CALDAV, "comp").filter_map(|c| c.attr("name")).collect();
                    if !comps.is_empty() && !comps.iter().any(|c| c.eq_ignore_ascii_case("VEVENT")) {
                        continue;
                    }
                }
                CollectionKind::Calendar
            } else if rt.child(CARDDAV, "addressbook").is_some() {
                CollectionKind::AddressBook
            } else {
                continue;
            };
            let url = join(&r.url, &href);
            let read_only = match prop.child(DAV, "current-user-privilege-set") {
                Some(ps) => !ps.children_of(DAV, "privilege").any(|p| {
                    p.child(DAV, "write").is_some() || p.child(DAV, "write-content").is_some() || p.child(DAV, "all").is_some()
                }),
                None => false,
            };
            let name = prop.text_of(DAV, "displayname").unwrap_or_else(|| {
                percent_decode(href.trim_end_matches('/').rsplit('/').next().unwrap_or("Calendar"))
            });
            let color = prop.text_of(APPLE, "calendar-color").map(|c| c.chars().take(7).collect::<String>()).filter(|c| c.starts_with('#') && c.len() == 7);
            out.push(Collection { url, kind, name, color, ctag: prop.text_of(CS, "getctag"), sync_token: prop.text_of(DAV, "sync-token"), read_only });
        }
        Ok(out)
    }

    /// Changes since `token` (RFC 6578). Falls back to a full etag listing when the server has no
    /// sync-collection support or the token expired.
    pub async fn delta(&self, coll: &Collection, token: Option<&str>) -> Result<Delta> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><d:sync-collection xmlns:d="DAV:"><d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"#,
            xml::esc(token.unwrap_or(""))
        );
        let r = self.request("REPORT", &coll.url, None, Some(body), "application/xml; charset=utf-8", &[]).await?;
        if r.status.as_u16() == 207 {
            if let Some(tree) = xml::parse(&r.body) {
                let mut d = Delta { full: token.is_none(), token: tree.text_of(DAV, "sync-token"), ..Default::default() };
                for resp in tree.children_of(DAV, "response") {
                    let Some(href) = resp.text_of(DAV, "href") else { continue };
                    if same_href(&join(&r.url, &href), &coll.url) {
                        continue;
                    }
                    let status = resp.text_of(DAV, "status").unwrap_or_default();
                    if status.contains(" 404") {
                        d.removed.push(join(&r.url, &href));
                    } else if let Some(p) = ok_prop(resp) {
                        d.changed.push((join(&r.url, &href), p.text_of(DAV, "getetag")));
                    }
                }
                if d.token.is_some() {
                    return Ok(d);
                }
            }
        }
        // Token rejected (403/409 valid-sync-token) or no RFC 6578: list every member with its etag.
        let r = self.propfind(&coll.url, "1", "<d:getetag/><d:resourcetype/>").await?;
        let tree = xml::parse(&r.body).ok_or_else(|| ApiError::server("The server sent a response WolfMail could not read."))?;
        let mut d = Delta { full: true, ..Default::default() };
        for resp in tree.children_of(DAV, "response") {
            let Some(href) = resp.text_of(DAV, "href") else { continue };
            let abs = join(&r.url, &href);
            if same_href(&abs, &coll.url) {
                continue;
            }
            if let Some(p) = ok_prop(resp) {
                if p.child(DAV, "resourcetype").is_some_and(|rt| rt.child(DAV, "collection").is_some()) {
                    continue;
                }
                d.changed.push((abs, p.text_of(DAV, "getetag")));
            }
        }
        Ok(d)
    }

    /// calendar-multiget / addressbook-multiget for up to 50 hrefs per request → (href, etag, data).
    pub async fn multiget(&self, coll: &Collection, hrefs: &[String]) -> Result<Vec<(String, Option<String>, String)>> {
        let mut out = Vec::new();
        for chunk in hrefs.chunks(50) {
            let (root, data_el, ns) = match coll.kind {
                CollectionKind::Calendar => ("c:calendar-multiget", "c:calendar-data", CALDAV),
                CollectionKind::AddressBook => ("card:addressbook-multiget", "card:address-data", CARDDAV),
            };
            let hrefs_xml: String = chunk
                .iter()
                .map(|h| format!("<d:href>{}</d:href>", xml::esc(url::Url::parse(h).map(|u| u.path().to_string()).as_deref().unwrap_or(h))))
                .collect();
            let body = format!(
                r#"<?xml version="1.0" encoding="utf-8"?><{root} xmlns:d="DAV:" xmlns:c="{CALDAV}" xmlns:card="{CARDDAV}"><d:prop><d:getetag/><{data_el}/></d:prop>{hrefs_xml}</{root}>"#
            );
            let r = self.request("REPORT", &coll.url, Some("1"), Some(body), "application/xml; charset=utf-8", &[]).await?;
            if r.status.as_u16() != 207 {
                return Err(ApiError::server(format!("The server answered the fetch with HTTP {}.", r.status.as_u16())));
            }
            let tree = xml::parse(&r.body).ok_or_else(|| ApiError::server("The server sent a response WolfMail could not read."))?;
            let data_name = if coll.kind == CollectionKind::Calendar { "calendar-data" } else { "address-data" };
            for resp in tree.children_of(DAV, "response") {
                let Some(href) = resp.text_of(DAV, "href") else { continue };
                let Some(p) = ok_prop(resp) else { continue };
                if let Some(data) = p.child(ns, data_name) {
                    out.push((join(&r.url, &href), p.text_of(DAV, "getetag"), data.text.clone()));
                }
            }
        }
        Ok(out)
    }

    /// PUT a member. New members use If-None-Match: * so an existing resource is never overwritten; updates use
    /// If-Match with the last known etag. Returns the new etag when the server sends one.
    pub async fn put(&self, url: &str, body: String, ctype: &str, etag: Option<&str>) -> Result<Option<String>> {
        let pre: Vec<(&str, &str)> = match etag {
            Some(e) => vec![("If-Match", e)],
            None => vec![("If-None-Match", "*")],
        };
        let r = self.request("PUT", url, None, Some(body), ctype, &pre).await?;
        match r.status.as_u16() {
            200..=299 => Ok(r.headers.get("etag").and_then(|v| v.to_str().ok()).map(|s| s.to_string())),
            412 => Err(ApiError::new(ErrorKind::Server, "CONFLICT: it was changed on the server since WolfMail last synced.")),
            403 => Err(ApiError::server("The server does not allow changes here (read-only).")),
            s => Err(ApiError::server(format!("The server refused the change (HTTP {s}). {}", short(&r.body)))),
        }
    }

    pub async fn delete(&self, url: &str, etag: Option<&str>) -> Result<()> {
        let pre: Vec<(&str, &str)> = etag.map(|e| vec![("If-Match", e)]).unwrap_or_default();
        let r = self.request("DELETE", url, None, None, "", &pre).await?;
        match r.status.as_u16() {
            200..=299 | 404 | 410 => Ok(()),
            412 => Err(ApiError::new(ErrorKind::Server, "CONFLICT: it was changed on the server since WolfMail last synced.")),
            s => Err(ApiError::server(format!("The server refused the delete (HTTP {s}).")))
        }
    }

    /// RFC 4791 §5.3.1 MKCALENDAR with a display name.
    pub async fn mkcalendar(&self, url: &str, name: &str) -> Result<()> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><c:mkcalendar xmlns:d="DAV:" xmlns:c="{CALDAV}"><d:set><d:prop><d:displayname>{}</d:displayname><c:supported-calendar-component-set><c:comp name="VEVENT"/></c:supported-calendar-component-set></d:prop></d:set></c:mkcalendar>"#,
            xml::esc(name)
        );
        let r = self.request("MKCALENDAR", url, None, Some(body), "application/xml; charset=utf-8", &[]).await?;
        if r.status.is_success() {
            Ok(())
        } else {
            Err(ApiError::server(format!("The server would not create the calendar (HTTP {}).", r.status.as_u16())))
        }
    }
}

fn short(s: &str) -> String {
    s.chars().take(160).collect()
}

/// The <prop> of the first 2xx <propstat> of a <response>.
fn ok_prop(resp: &El) -> Option<&El> {
    resp.children_of(DAV, "propstat")
        .find(|ps| ps.text_of(DAV, "status").is_none_or(|s| s.contains(" 2")))
        .and_then(|ps| ps.child(DAV, "prop"))
}

fn collect_hrefs(tree: &El, ns: &str, name: &str, base: &str, out: &mut Vec<String>) {
    if let Some(el) = tree.find(ns, name) {
        for h in el.children_of(DAV, "href") {
            let v = h.text.trim();
            if !v.is_empty() {
                let abs = join(base, v);
                if !out.iter().any(|x| same_href(x, &abs)) {
                    out.push(abs);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn href_normalisation() {
        assert!(same_href("https://x.org/cal/a%20b/", "/cal/a b"));
        assert!(!same_href("/cal/a", "/cal/b"));
        assert_eq!(join("https://x.org/dav/cal/", "/dav/cal/e.ics"), "https://x.org/dav/cal/e.ics");
    }

    #[test]
    fn sync_collection_response_shape() {
        let body = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/c/new.ics</d:href><d:propstat><d:prop><d:getetag>"e1"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:response><d:href>/c/gone.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response><d:sync-token>http://x/sync/2</d:sync-token></d:multistatus>"#;
        let t = xml::parse(body).unwrap();
        assert_eq!(t.text_of(DAV, "sync-token").as_deref(), Some("http://x/sync/2"));
        let resps: Vec<_> = t.children_of(DAV, "response").collect();
        assert_eq!(ok_prop(resps[0]).unwrap().text_of(DAV, "getetag").as_deref(), Some("\"e1\""));
        assert!(resps[1].text_of(DAV, "status").unwrap().contains(" 404"));
    }
}
