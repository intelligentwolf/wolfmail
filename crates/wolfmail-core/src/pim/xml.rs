//! A tiny namespace-resolved XML tree for WebDAV multistatus responses.
//! Source: quick-xml 0.42 `NsReader::read_resolved_event` (src/reader/ns_reader.rs:582), `Event::{Text, CData,
//! GeneralRef}` (src/events/mod.rs:1784-1803) — 0.42 reports entity references as separate `GeneralRef` events, so
//! predefined entities (XML 1.0 §4.6) and character references are resolved here.
use quick_xml::events::Event as X;
use quick_xml::name::ResolveResult;
use quick_xml::NsReader;

pub const DAV: &str = "DAV:";
pub const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
pub const CARDDAV: &str = "urn:ietf:params:xml:ns:carddav";
pub const CS: &str = "http://calendarserver.org/ns/";
pub const APPLE: &str = "http://apple.com/ns/ical/";

#[derive(Debug, Clone, Default)]
pub struct El {
    pub ns: String,
    pub name: String,
    pub text: String,
    /// Attributes by local name (namespaces on attributes are not needed for DAV).
    pub attrs: Vec<(String, String)>,
    pub children: Vec<El>,
}

impl El {
    pub fn is(&self, ns: &str, name: &str) -> bool {
        self.ns == ns && self.name == name
    }
    pub fn child(&self, ns: &str, name: &str) -> Option<&El> {
        self.children.iter().find(|c| c.is(ns, name))
    }
    pub fn children_of<'a>(&'a self, ns: &'a str, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.children.iter().filter(move |c| c.is(ns, name))
    }
    /// Depth-first search.
    pub fn find(&self, ns: &str, name: &str) -> Option<&El> {
        if self.is(ns, name) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(ns, name))
    }
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    pub fn text_of(&self, ns: &str, name: &str) -> Option<String> {
        self.child(ns, name).map(|c| c.text.trim().to_string()).filter(|s| !s.is_empty())
    }
}

fn predefined(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

fn attrs_of(s: &quick_xml::events::BytesStart) -> Vec<(String, String)> {
    s.attributes()
        .flatten()
        .map(|a| (a.key.local_name().as_ref().to_string(), a.normalized_value(quick_xml::XmlVersion::Implicit1_0).map(|v| v.into_owned()).unwrap_or_default()))
        .collect()
}

pub fn parse(xml: &str) -> Option<El> {
    let mut r = NsReader::from_str(xml);
    let mut stack: Vec<El> = Vec::new();
    loop {
        let (res, ev) = r.read_resolved_event().ok()?;
        let ns_of = |res: &ResolveResult| match res {
            ResolveResult::Bound(ns) => ns.0.to_string(),
            _ => String::new(),
        };
        match ev {
            X::Start(s) => {
                let ns = ns_of(&res);
                stack.push(El { ns, name: s.local_name().as_ref().to_string(), attrs: attrs_of(&s), ..Default::default() });
            }
            X::Empty(s) => {
                let el = El { ns: ns_of(&res), name: s.local_name().as_ref().to_string(), attrs: attrs_of(&s), ..Default::default() };
                match stack.last_mut() {
                    Some(p) => p.children.push(el),
                    None => return Some(el),
                }
            }
            X::End(_) => {
                let done = stack.pop()?;
                match stack.last_mut() {
                    Some(p) => p.children.push(done),
                    None => return Some(done),
                }
            }
            X::Text(t) => {
                if let Some(cur) = stack.last_mut() {
                    cur.text.push_str(&t.html_content());
                }
            }
            X::CData(t) => {
                if let Some(cur) = stack.last_mut() {
                    cur.text.push_str(&t.html_content());
                }
            }
            X::GeneralRef(g) => {
                if let Some(cur) = stack.last_mut() {
                    if let Ok(Some(ch)) = g.resolve_char_ref() {
                        cur.text.push(ch);
                    } else if let Some(ch) = predefined(&g.html_content()) {
                        cur.text.push(ch);
                    }
                }
            }
            X::Eof => return stack.into_iter().next(),
            _ => {}
        }
    }
}

/// Escape text for an XML element body.
pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_namespaces_entities_and_cdata() {
        let x = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:response><d:href>/cal/a%20b.ics</d:href><d:propstat><d:prop><c:calendar-data><![CDATA[BEGIN:VCALENDAR]]>&#10;A &amp; B</c:calendar-data></d:prop></d:propstat></d:response></d:multistatus>"#;
        let t = parse(x).unwrap();
        assert!(t.is(DAV, "multistatus"));
        let data = t.find(CALDAV, "calendar-data").unwrap();
        assert_eq!(data.text, "BEGIN:VCALENDAR\nA & B");
        assert_eq!(t.find(DAV, "href").unwrap().text, "/cal/a%20b.ics");
    }
}
