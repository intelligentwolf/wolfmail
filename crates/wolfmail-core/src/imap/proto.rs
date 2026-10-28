//! Typed views over raw IMAP responses: SELECT results, FETCH rows, LIST entries.
// Source: RFC 3501 §7 (responses), RFC 7162 §3.1.2/§3.2.10 (HIGHESTMODSEQ, VANISHED), RFC 6154 (SPECIAL-USE),
// Google "IMAP Extensions" (X-GM-THRID / X-GM-MSGID / X-GM-LABELS) — parsed by imap-proto 0.17.
use super::conn::{quote, Conn};
use crate::error::Result;
use super::conn::Resp;
use async_imap::imap_proto::{self, AttributeValue, BodyStructure, MailboxDatum, NameAttribute, Response, ResponseCode};
use std::ops::RangeInclusive;

#[derive(Debug, Default, Clone)]
pub struct SelectInfo {
    pub exists: u32,
    pub uidvalidity: Option<u32>,
    pub uidnext: Option<u32>,
    pub highest_modseq: Option<u64>,
    pub nomodseq: bool,
    pub vanished: Vec<RangeInclusive<u32>>,
    pub changed: Vec<FetchRow>,
    pub read_only: bool,
}

#[derive(Debug, Default, Clone)]
pub struct FetchRow {
    pub seq: u32,
    pub uid: Option<u32>,
    pub flags: Option<Vec<String>>,
    pub modseq: Option<u64>,
    pub size: Option<u32>,
    pub internal_date: Option<String>,
    pub header: Option<Vec<u8>>,
    pub body: Option<Vec<u8>>,
    pub has_attachments: Option<bool>,
    pub gm_thrid: Option<u64>,
    pub gm_msgid: Option<u64>,
    pub gm_labels: Option<Vec<String>>,
}

fn row_from(seq: u32, attrs: &[AttributeValue<'_>]) -> FetchRow {
    let mut r = FetchRow { seq, ..Default::default() };
    for a in attrs {
        match a {
            AttributeValue::Uid(u) => r.uid = Some(*u),
            AttributeValue::Flags(f) => r.flags = Some(f.iter().map(|x| x.to_string()).collect()),
            AttributeValue::ModSeq(m) => r.modseq = Some(*m),
            AttributeValue::Rfc822Size(s) => r.size = Some(*s),
            AttributeValue::InternalDate(d) => r.internal_date = Some(d.to_string()),
            AttributeValue::BodySection { section, data, .. } => {
                let bytes = data.as_ref().map(|d| d.to_vec());
                match section {
                    None => r.body = bytes,
                    Some(_) => r.header = bytes,
                }
            }
            AttributeValue::Rfc822(d) => r.body = d.as_ref().map(|d| d.to_vec()),
            AttributeValue::Rfc822Header(d) => r.header = d.as_ref().map(|d| d.to_vec()),
            AttributeValue::BodyStructure(bs) => r.has_attachments = Some(structure_has_attachments(bs)),
            AttributeValue::GmailThrId(t) => r.gm_thrid = Some(*t),
            AttributeValue::GmailMsgId(m) => r.gm_msgid = Some(*m),
            AttributeValue::GmailLabels(l) => r.gm_labels = Some(l.iter().map(|x| x.to_string()).collect()),
            _ => {}
        }
    }
    r
}

fn param<'a>(params: &'a imap_proto::BodyParams<'a>, key: &str) -> Option<&'a str> {
    params.as_ref()?.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_ref())
}

/// A part is a user-visible attachment when its disposition says so, when it is a forwarded message, or when it
/// is a named non-text part that is not an inline image referenced by Content-ID.
pub fn structure_has_attachments(bs: &BodyStructure<'_>) -> bool {
    match bs {
        BodyStructure::Multipart { bodies, .. } => bodies.iter().any(structure_has_attachments),
        BodyStructure::Message { .. } => true,
        BodyStructure::Text { common, .. } => common.disposition.as_ref().is_some_and(|d| d.ty.eq_ignore_ascii_case("attachment")),
        BodyStructure::Basic { common, other, .. } => {
            let disp = common.disposition.as_ref().map(|d| d.ty.to_ascii_lowercase());
            if disp.as_deref() == Some("attachment") {
                return true;
            }
            let is_image = common.ty.ty.eq_ignore_ascii_case("image");
            if is_image && other.id.is_some() {
                return false;
            }
            disp.as_deref() != Some("inline")
                || param(&common.ty.params, "name").is_some()
                || common.disposition.as_ref().and_then(|d| param(&d.params, "filename")).is_some()
        }
    }
}

pub fn parse_fetch_rows(resps: &[Resp]) -> Vec<FetchRow> {
    resps
        .iter()
        .filter_map(|r| match r.parsed() {
            Response::Fetch(seq, attrs) => Some(row_from(*seq, attrs)),
            _ => None,
        })
        .collect()
}

pub fn parse_vanished(resps: &[Resp]) -> Vec<RangeInclusive<u32>> {
    resps
        .iter()
        .filter_map(|r| match r.parsed() {
            Response::Vanished { uids, .. } => Some(uids.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn apply_code(info: &mut SelectInfo, code: &ResponseCode<'_>) {
    match code {
        ResponseCode::UidValidity(v) => info.uidvalidity = Some(*v),
        ResponseCode::UidNext(v) => info.uidnext = Some(*v),
        ResponseCode::HighestModSeq(v) => info.highest_modseq = Some(*v),
        ResponseCode::ReadOnly => info.read_only = true,
        _ => {}
    }
}

pub fn parse_select(resps: &[Resp]) -> SelectInfo {
    let mut info = SelectInfo::default();
    for r in resps {
        match r.parsed() {
            Response::MailboxData(MailboxDatum::Exists(n)) => info.exists = *n,
            Response::Data { outcome, .. } | Response::Done { outcome, .. } => {
                if let Some(code) = &outcome.code {
                    apply_code(&mut info, code);
                }
                // Source: RFC 7162 §3.1.2.2 — "* OK [NOMODSEQ]" when the mailbox does not support mod-sequences.
                // imap-proto has no variant for it, so it arrives as text information.
                if outcome.information.as_deref().is_some_and(|i| i.to_ascii_uppercase().contains("NOMODSEQ")) {
                    info.nomodseq = true;
                }
            }
            Response::Vanished { uids, .. } => info.vanished.extend(uids.iter().cloned()),
            Response::Fetch(seq, attrs) => info.changed.push(row_from(*seq, attrs)),
            _ => {}
        }
    }
    info
}

#[derive(Debug, Clone)]
pub struct ListEntry {
    pub path: String,
    pub delimiter: Option<String>,
    pub selectable: bool,
    pub special: Option<crate::model::FolderRole>,
}

pub fn parse_list(resps: &[Resp]) -> Vec<ListEntry> {
    use crate::model::FolderRole as R;
    resps
        .iter()
        .filter_map(|r| match r.parsed() {
            Response::MailboxData(MailboxDatum::List(l)) => {
                let mut selectable = true;
                let mut special = None;
                for a in &l.name_attributes {
                    match a {
                        NameAttribute::NoSelect => selectable = false,
                        NameAttribute::All => special = Some(R::All),
                        NameAttribute::Archive => special = Some(R::Archive),
                        NameAttribute::Drafts => special = Some(R::Drafts),
                        NameAttribute::Flagged => special = Some(R::Flagged),
                        NameAttribute::Junk => special = Some(R::Junk),
                        NameAttribute::Sent => special = Some(R::Sent),
                        NameAttribute::Trash => special = Some(R::Trash),
                        NameAttribute::Extension(e) => {
                            let e = e.to_ascii_lowercase();
                            if e == "\\nonexistent" {
                                selectable = false;
                            }
                        }
                        _ => {}
                    }
                }
                Some(ListEntry {
                    path: l.name.to_string(),
                    delimiter: l.delimiter.as_ref().map(|d| d.to_string()),
                    selectable,
                    special,
                })
            }
            _ => None,
        })
        .collect()
}

pub fn parse_search(resps: &[Resp]) -> Vec<u32> {
    resps
        .iter()
        .filter_map(|r| match r.parsed() {
            Response::MailboxData(MailboxDatum::Search(v)) => Some(v.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// COPYUID from a MOVE/COPY tagged OK (RFC 4315 §3 / RFC 6851 §4.3): (uidvalidity, source uids, dest uids).
pub fn parse_copyuid(resps: &[Resp]) -> Option<(Vec<u32>, Vec<u32>)> {
    use async_imap::imap_proto::rfc4315::UidSetMember;
    let expand = |set: &[UidSetMember]| -> Vec<u32> {
        set.iter()
            .flat_map(|m| match m {
                UidSetMember::Uid(u) => vec![*u],
                UidSetMember::UidRange(r) => r.clone().collect(),
            })
            .collect()
    };
    for r in resps {
        let outcome = match r.parsed() {
            Response::Data { outcome, .. } | Response::Done { outcome, .. } => outcome,
            _ => continue,
        };
        if let Some(ResponseCode::CopyUid(_, src, dst)) = &outcome.code {
            return Some((expand(src), expand(dst)));
        }
    }
    None
}

pub fn parse_appenduid(resps: &[Resp]) -> Option<u32> {
    use async_imap::imap_proto::rfc4315::UidSetMember;
    for r in resps {
        if let Response::Done { outcome, .. } = r.parsed() {
            if let Some(ResponseCode::AppendUid(_, set)) = &outcome.code {
                if let Some(UidSetMember::Uid(u)) = set.first() {
                    return Some(*u);
                }
            }
        }
    }
    None
}

/// Compact a sorted list of UIDs into an IMAP sequence set ("1:5,9,12:14").
pub fn uid_set(uids: &[u32]) -> String {
    let mut v = uids.to_vec();
    v.sort_unstable();
    v.dedup();
    let mut out = String::new();
    let mut i = 0;
    while i < v.len() {
        let start = v[i];
        let mut end = start;
        while i + 1 < v.len() && v[i + 1] == end + 1 {
            i += 1;
            end = v[i];
        }
        if !out.is_empty() {
            out.push(',');
        }
        if start == end {
            out.push_str(&start.to_string());
        } else {
            out.push_str(&format!("{start}:{end}"));
        }
        i += 1;
    }
    out
}

impl Conn {
    /// SELECT (or re-SELECT) a mailbox. With QRESYNC enabled and known state, asks for changes since `known`.
    pub async fn select(&mut self, path: &str, known: Option<(u32, u64)>) -> Result<SelectInfo> {
        let q = quote(path);
        let cmd = match (self.qresync, known) {
            // Source: RFC 7162 §3.2.5 — SELECT mailbox (QRESYNC (uidvalidity modseq))
            (true, Some((uv, ms))) if ms > 0 => format!("SELECT {q} (QRESYNC ({uv} {ms}))"),
            _ if self.condstore() => format!("SELECT {q} (CONDSTORE)"),
            _ => format!("SELECT {q}"),
        };
        let resps = self.raw(&cmd).await;
        match resps {
            Ok(r) => {
                self.selected = Some(path.to_string());
                Ok(parse_select(&r))
            }
            Err(e) => {
                self.selected = None;
                Err(e)
            }
        }
    }

    pub async fn ensure_selected(&mut self, path: &str) -> Result<()> {
        if self.selected.as_deref() != Some(path) {
            self.select(path, None).await?;
        }
        Ok(())
    }

    pub async fn uid_fetch(&mut self, set: &str, items: &str, modifiers: Option<&str>) -> Result<(Vec<FetchRow>, Vec<RangeInclusive<u32>>)> {
        let cmd = match modifiers {
            Some(m) => format!("UID FETCH {set} {items} {m}"),
            None => format!("UID FETCH {set} {items}"),
        };
        let r = self.raw(&cmd).await?;
        Ok((parse_fetch_rows(&r), parse_vanished(&r)))
    }

    pub async fn list_all(&mut self) -> Result<Vec<ListEntry>> {
        let r = self.raw("LIST \"\" \"*\"").await?;
        Ok(parse_list(&r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uid_sets() {
        assert_eq!(uid_set(&[5, 1, 2, 3, 9, 12, 13, 14]), "1:3,5,9,12:14");
        assert_eq!(uid_set(&[7]), "7");
        assert_eq!(uid_set(&[]), "");
    }
}
