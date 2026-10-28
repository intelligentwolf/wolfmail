//! Message rows: insert from a header FETCH, flag updates, deletes, view queries, full-message assembly.
// Source: docs/API.md MessageSummary / ViewQuery / MessagePage.
use crate::db::now_ms;
use crate::error::{ApiError, Result};
use crate::mime::{self, HeaderInfo};
use crate::model::*;
use crate::threading::{assign_thread, ThreadInput};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};

/// Keywords that are protocol state, not user tags. Source: RFC 3501 §2.3.2 system flags; RFC 5788 registry
/// ($Forwarded, $MDNSent, $Junk, $NotJunk, $Phishing); Thunderbird's NonJunk/Junk.
const NON_TAG_KEYWORDS: &[&str] = &["$forwarded", "forwarded", "$mdnsent", "$junk", "$notjunk", "junk", "nonjunk", "notjunk", "$phishing", "$submitpending", "$submitted", "$hasattachment", "$hasnoattachment"];

#[derive(Debug, Default, Clone)]
pub struct ParsedFlags {
    pub flags: Flags,
    pub deleted: bool,
    pub keywords: Vec<String>,
}

pub fn parse_flags(raw: &[String]) -> ParsedFlags {
    let mut p = ParsedFlags::default();
    for f in raw {
        let l = f.to_ascii_lowercase();
        match l.as_str() {
            "\\seen" => p.flags.seen = true,
            "\\flagged" => p.flags.flagged = true,
            "\\answered" => p.flags.answered = true,
            "\\draft" => p.flags.draft = true,
            "\\deleted" => p.deleted = true,
            "$forwarded" | "forwarded" => p.flags.forwarded = true,
            _ if l.starts_with('\\') => {}
            _ if NON_TAG_KEYWORDS.contains(&l.as_str()) => {}
            _ => p.keywords.push(f.clone()),
        }
    }
    p
}

pub struct NewMessage<'a> {
    pub account_id: &'a str,
    pub folder_id: i64,
    pub uid: u32,
    pub header: HeaderInfo,
    pub flags: ParsedFlags,
    pub size: i64,
    pub internal_date: Option<i64>,
    pub has_attachments: bool,
    pub modseq: Option<u64>,
    pub gm_thrid: Option<u64>,
    pub gm_msgid: Option<u64>,
    /// Provider thread id (Gmail API threadId / Graph conversationId), see threading::ThreadInput::ext_thread.
    pub ext_thread: Option<String>,
}

/// Insert (or replace on (folder, uid)) a message row. Returns the rowid.
pub fn insert_message(conn: &Connection, m: &NewMessage<'_>) -> Result<i64> {
    let date = m.header.date.or(m.internal_date).unwrap_or_else(now_ms);
    let thread_id = assign_thread(
        conn,
        &ThreadInput {
            account_id: m.account_id,
            message_id: m.header.message_id.as_deref(),
            in_reply_to: m.header.in_reply_to.as_deref(),
            references: &m.header.references,
            subject: &m.header.subject,
            date,
            gm_thrid: m.gm_thrid,
            ext_thread: m.ext_thread.as_deref(),
        },
    )?;
    let existing: Option<i64> = conn
        .query_row("SELECT id FROM messages WHERE folder_id=?1 AND uid=?2", params![m.folder_id, m.uid], |r| r.get(0))
        .optional()?;
    if let Some(id) = existing {
        conn.execute("DELETE FROM messages_fts WHERE rowid=?1", params![id])?;
        conn.execute("DELETE FROM messages WHERE id=?1", params![id])?;
    }
    conn.execute(
        "INSERT INTO messages(account_id, folder_id, uid, message_id, thread_id, in_reply_to, refs, subject, base_subject,
            from_name, from_email, to_json, cc_json, bcc_json, reply_to_json, date, seen, flagged, answered, forwarded, draft,
            keywords, has_attachments, size, modseq, gm_thrid, gm_msgid, list_unsub_json, auth_json)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29)",
        params![
            m.account_id,
            m.folder_id,
            m.uid,
            m.header.message_id,
            thread_id,
            m.header.in_reply_to,
            serde_json::to_string(&m.header.references)?,
            m.header.subject,
            mime::base_subject(&m.header.subject),
            m.header.from.name,
            m.header.from.email.to_ascii_lowercase(),
            serde_json::to_string(&m.header.to)?,
            serde_json::to_string(&m.header.cc)?,
            serde_json::to_string(&m.header.bcc)?,
            serde_json::to_string(&m.header.reply_to)?,
            date,
            m.flags.flags.seen,
            m.flags.flags.flagged,
            m.flags.flags.answered,
            m.flags.flags.forwarded,
            m.flags.flags.draft,
            serde_json::to_string(&m.flags.keywords)?,
            m.has_attachments,
            m.size,
            m.modseq.map(|v| v as i64),
            m.gm_thrid.map(|v| v as i64),
            m.gm_msgid.map(|v| v as i64),
            m.header.list_unsubscribe.as_ref().map(serde_json::to_string).transpose()?,
            m.header.auth_results.as_ref().map(serde_json::to_string).transpose()?,
        ],
    )?;
    let id = conn.last_insert_rowid();
    let recipients = m.header.to.iter().chain(m.header.cc.iter()).map(addr_text).collect::<Vec<_>>().join(" ");
    conn.execute(
        "INSERT INTO messages_fts(rowid, subject, sender, recipients, body) VALUES (?1, ?2, ?3, ?4, '')",
        params![id, m.header.subject, addr_text(&m.header.from), recipients],
    )?;
    note_contact(conn, &m.header.from, date, false)?;
    Ok(id)
}

fn addr_text(a: &Address) -> String {
    match &a.name {
        Some(n) => format!("{n} {}", a.email),
        None => a.email.clone(),
    }
}

pub fn note_contact(conn: &Connection, a: &Address, at: i64, sent_to: bool) -> Result<()> {
    if a.email.is_empty() || !a.email.contains('@') {
        return Ok(());
    }
    let email = a.email.to_ascii_lowercase();
    if email.starts_with("no-reply") || email.starts_with("noreply") || email.starts_with("donotreply") || email.starts_with("mailer-daemon") {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO contacts_seen(email, name, count, last_seen, sent_to) VALUES (?1, ?2, 1, ?3, ?4)
         ON CONFLICT(email) DO UPDATE SET count=count+1, last_seen=MAX(last_seen, excluded.last_seen),
            name=COALESCE(excluded.name, contacts_seen.name), sent_to=MAX(sent_to, excluded.sent_to)",
        params![email, a.name, at, sent_to as i64],
    )?;
    Ok(())
}

/// Store the raw body, attachments and the FTS body text; update snippet / has_attachments.
pub fn store_body(conn: &Connection, rowid: i64, raw: &[u8]) -> Result<()> {
    let parsed = mime::parse_body(raw, false);
    conn.execute("INSERT OR REPLACE INTO bodies(message_rowid, raw) VALUES (?1, ?2)", params![rowid, raw])?;
    conn.execute("DELETE FROM attachments WHERE message_rowid=?1", params![rowid])?;
    for a in &parsed.attachments {
        conn.execute(
            "INSERT INTO attachments(message_rowid, part_id, filename, mime, size, inline, content_id) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![rowid, a.id, a.filename, a.mime, a.size, a.inline, a.content_id],
        )?;
    }
    conn.execute(
        "UPDATE messages SET body_cached=1, snippet=?2, has_attachments=?3 WHERE id=?1",
        params![rowid, parsed.snippet, parsed.has_attachments],
    )?;
    let (subject, sender, recipients): (String, String, String) = conn.query_row(
        "SELECT subject, COALESCE(from_name,'') || ' ' || from_email, to_json || cc_json FROM messages WHERE id=?1",
        params![rowid],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let body: String = parsed.index_text.chars().take(200_000).collect();
    conn.execute("DELETE FROM messages_fts WHERE rowid=?1", params![rowid])?;
    conn.execute(
        "INSERT INTO messages_fts(rowid, subject, sender, recipients, body) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![rowid, subject, sender, recipients, body],
    )?;
    Ok(())
}

pub fn update_flags(conn: &Connection, folder_id: i64, uid: u32, f: &ParsedFlags, modseq: Option<u64>) -> Result<()> {
    if f.deleted {
        return delete_uid(conn, folder_id, uid);
    }
    conn.execute(
        "UPDATE messages SET seen=?3, flagged=?4, answered=?5, forwarded=?6, draft=?7, keywords=?8, modseq=COALESCE(?9, modseq)
         WHERE folder_id=?1 AND uid=?2",
        params![
            folder_id,
            uid,
            f.flags.seen,
            f.flags.flagged,
            f.flags.answered,
            f.flags.forwarded,
            f.flags.draft,
            serde_json::to_string(&f.keywords)?,
            modseq.map(|v| v as i64)
        ],
    )?;
    Ok(())
}

pub fn delete_uid(conn: &Connection, folder_id: i64, uid: u32) -> Result<()> {
    if let Some(id) = conn
        .query_row("SELECT id FROM messages WHERE folder_id=?1 AND uid=?2", params![folder_id, uid], |r| r.get::<_, i64>(0))
        .optional()?
    {
        delete_row(conn, id)?;
    }
    Ok(())
}

pub fn delete_row(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM messages_fts WHERE rowid=?1", params![id])?;
    conn.execute("DELETE FROM messages WHERE id=?1", params![id])?;
    Ok(())
}

/// Columns 0–20 read by `row_to_summary` (20 = snoozed_until). Queries that add their own columns after these
/// start reading at index 21.
pub const SUMMARY_COLS: &str = "m.id, m.account_id, m.folder_id, m.thread_id, m.message_id, m.subject, m.from_name, m.from_email,
    m.to_json, m.cc_json, m.date, m.snippet, m.seen, m.flagged, m.answered, m.forwarded, m.draft, m.keywords,
    m.has_attachments, m.size, (SELECT sz.until_ms FROM snoozes sz WHERE sz.message_rowid = m.id)";

pub fn row_to_summary(r: &Row<'_>, thread_count: i64, thread_unread: i64) -> rusqlite::Result<MessageSummary> {
    let kw: String = r.get(17)?;
    let keywords: Vec<String> = serde_json::from_str(&kw).unwrap_or_default();
    let to: String = r.get(8)?;
    let cc: String = r.get(9)?;
    Ok(MessageSummary {
        id: r.get::<_, i64>(0)?.to_string(),
        account_id: r.get(1)?,
        folder_id: r.get::<_, i64>(2)?.to_string(),
        thread_id: r.get(3)?,
        message_id: r.get(4)?,
        subject: r.get(5)?,
        from: Address { name: r.get(6)?, email: r.get(7)? },
        to: serde_json::from_str(&to).unwrap_or_default(),
        cc: serde_json::from_str(&cc).unwrap_or_default(),
        date: r.get(10)?,
        snippet: r.get(11)?,
        flags: Flags { seen: r.get(12)?, flagged: r.get(13)?, answered: r.get(14)?, forwarded: r.get(15)?, draft: r.get(16)? },
        tags: keywords,
        has_attachments: r.get(18)?,
        size: r.get(19)?,
        thread_count,
        thread_unread,
        snoozed_until: r.get(20)?,
    })
}

pub fn get_summary(conn: &Connection, id: i64) -> Result<MessageSummary> {
    conn.query_row(&format!("SELECT {SUMMARY_COLS} FROM messages m WHERE m.id=?1"), params![id], |r| row_to_summary(r, 1, 0))
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => ApiError::not_found("The message"),
            other => other.into(),
        })
}

/// True when no Gmail label puts the message in Trash or Spam. Gmail keeps a trashed message's other labels, but
/// it is hidden from them (and from All Mail) — Source: Gmail API labels guide, TRASH / SPAM are labels like any
/// other. IMAP and Graph rows have no message_labels, so this is always true for them.
pub const NOT_IN_TRASH_LABEL: &str = "NOT EXISTS (SELECT 1 FROM message_labels xl JOIN folders xf ON xf.id = xl.folder_id
    WHERE xl.message_rowid = m.id AND xf.role IN ('trash','junk'))";

/// Membership in any folder whose role is NOT in `excluded` (an SQL list of quoted roles), counting both the row's
/// own folder and its Gmail label folders.
fn member_of_roles_excluding(excluded: &str) -> String {
    format!(
        "(m.folder_id IN (SELECT id FROM folders WHERE COALESCE(role,'') NOT IN ({excluded}))
          OR m.id IN (SELECT ml.message_rowid FROM message_labels ml JOIN folders f ON f.id = ml.folder_id
                      WHERE COALESCE(f.role,'') NOT IN ({excluded})))"
    )
}

/// Build the WHERE clause for a view (folder or unified role) plus filters. Returns (sql, params).
pub fn view_where(q: &ViewQuery) -> Result<(String, Vec<rusqlite::types::Value>)> {
    use rusqlite::types::Value as V;
    let mut clauses: Vec<String> = Vec::new();
    let mut ps: Vec<V> = Vec::new();
    match (&q.folder_id, &q.unified) {
        (Some(fid), _) => {
            let fid: i64 = fid.parse().map_err(|_| ApiError::invalid("folderId", "Unknown folder."))?;
            // A Gmail-API message belongs to label folders through message_labels (one row per message).
            clauses.push("(m.folder_id = ? OR m.id IN (SELECT message_rowid FROM message_labels WHERE folder_id = ?))".into());
            ps.push(V::Integer(fid));
            ps.push(V::Integer(fid));
            clauses.push(format!("((SELECT COALESCE(role,'') FROM folders WHERE id = ?) IN ('trash','junk') OR {NOT_IN_TRASH_LABEL})"));
            ps.push(V::Integer(fid));
        }
        (None, Some(u)) => {
            let enabled = "m.account_id IN (SELECT id FROM accounts WHERE enabled=1)";
            clauses.push(enabled.into());
            match u {
                UnifiedView::Flagged => clauses.push(format!(
                    "m.flagged=1 AND {} AND {NOT_IN_TRASH_LABEL}",
                    member_of_roles_excluding("'trash','junk','all','flagged'")
                )),
                UnifiedView::Snoozed => clauses.push("m.id IN (SELECT message_rowid FROM snoozes)".into()),
                UnifiedView::Unread => clauses.push(format!(
                    "m.seen=0 AND {} AND {NOT_IN_TRASH_LABEL}",
                    member_of_roles_excluding("'trash','junk','all','sent','drafts','flagged'")
                )),
                other => {
                    let role = match other {
                        UnifiedView::Inbox => "inbox",
                        UnifiedView::Sent => "sent",
                        UnifiedView::Drafts => "drafts",
                        UnifiedView::Trash => "trash",
                        UnifiedView::Junk => "junk",
                        UnifiedView::Archive => "archive",
                        _ => unreachable!("handled above"),
                    };
                    clauses.push(
                        "(m.folder_id IN (SELECT id FROM folders WHERE role = ?)
                          OR m.id IN (SELECT ml.message_rowid FROM message_labels ml JOIN folders f ON f.id = ml.folder_id WHERE f.role = ?))"
                            .into(),
                    );
                    ps.push(V::Text(role.into()));
                    ps.push(V::Text(role.into()));
                    if role != "trash" && role != "junk" {
                        clauses.push(NOT_IN_TRASH_LABEL.into());
                    }
                }
            }
        }
        (None, None) => return Err(ApiError::invalid("folderId", "Choose a folder or a unified view.")),
    }
    // Snoozed messages are hidden everywhere except the Snoozed view until they come back.
    if !matches!(q.unified, Some(UnifiedView::Snoozed)) {
        clauses.push("m.id NOT IN (SELECT message_rowid FROM snoozes)".into());
    }
    if let Some(f) = &q.filter {
        if f.unread == Some(true) {
            clauses.push("m.seen=0".into());
        }
        if f.flagged == Some(true) {
            clauses.push("m.flagged=1".into());
        }
        if f.attachments == Some(true) {
            clauses.push("m.has_attachments=1".into());
        }
        if let Some(t) = &f.tag {
            clauses.push("EXISTS (SELECT 1 FROM json_each(m.keywords) j WHERE lower(j.value) = lower(?))".into());
            ps.push(V::Text(t.clone()));
        }
        if let Some(c) = &f.from_contact {
            clauses.push("m.from_email = lower(?)".into());
            ps.push(V::Text(c.clone()));
        }
    }
    if let Some(s) = q.search.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let parsed = crate::search::parse_query(s);
        let (sql, extra) = parsed.to_sql();
        if !sql.is_empty() {
            clauses.push(sql);
            ps.extend(extra);
        }
    }
    Ok((clauses.join(" AND "), ps))
}

pub fn list_view(conn: &Connection, q: &ViewQuery) -> Result<MessagePage> {
    let (where_sql, ps) = view_where(q)?;
    let order = match q.sort {
        SortOrder::DateDesc => "DESC",
        SortOrder::DateAsc => "ASC",
    };
    let limit = q.limit.clamp(1, 1000);
    let offset = q.offset.max(0);
    if q.threaded {
        // One row per thread (its newest message in the view), with counts over the thread's messages in the view.
        let total: i64 = conn.query_row(
            &format!("SELECT COUNT(DISTINCT m.account_id || '|' || m.thread_id) FROM messages m WHERE {where_sql}"),
            params_from_iter(ps.iter()),
            |r| r.get(0),
        )?;
        let sql = format!(
            "WITH v AS (SELECT m.* FROM messages m WHERE {where_sql}),
                  t AS (SELECT account_id, thread_id, COUNT(*) AS n, SUM(seen=0) AS unread, MAX(date) AS last,
                               (SELECT id FROM v v2 WHERE v2.account_id=v.account_id AND v2.thread_id=v.thread_id ORDER BY date DESC, id DESC LIMIT 1) AS top
                        FROM v GROUP BY account_id, thread_id)
             SELECT {SUMMARY_COLS}, t.n, t.unread FROM t JOIN messages m ON m.id = t.top
             ORDER BY t.last {order}, m.id {order} LIMIT {limit} OFFSET {offset}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let items = stmt
            .query_map(params_from_iter(ps.iter()), |r| {
                let n: i64 = r.get(21)?;
                let u: i64 = r.get(22)?;
                row_to_summary(r, n, u)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(MessagePage { total, items })
    } else {
        let total: i64 =
            conn.query_row(&format!("SELECT COUNT(*) FROM messages m WHERE {where_sql}"), params_from_iter(ps.iter()), |r| r.get(0))?;
        let sql = format!(
            "SELECT {SUMMARY_COLS} FROM messages m WHERE {where_sql} ORDER BY m.date {order}, m.id {order} LIMIT {limit} OFFSET {offset}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let items = stmt
            .query_map(params_from_iter(ps.iter()), |r| row_to_summary(r, 1, if r.get::<_, bool>(12)? { 0 } else { 1 }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(MessagePage { total, items })
    }
}

pub fn thread_messages(conn: &Connection, account_id: &str, thread_id: &str) -> Result<Vec<MessageSummary>> {
    // Every copy of the thread across folders except Trash/Junk, de-duplicated by Message-ID (Gmail "All Mail"
    // holds a second copy of everything — Betterbird patch 28 fixes the same duplication in its search).
    let mut stmt = conn.prepare(&format!(
        "SELECT {SUMMARY_COLS}, COALESCE(f.role,'') AS role FROM messages m JOIN folders f ON f.id = m.folder_id
         WHERE m.account_id=?1 AND m.thread_id=?2 AND COALESCE(f.role,'') NOT IN ('trash','junk') AND {NOT_IN_TRASH_LABEL}
         ORDER BY m.date ASC, m.id ASC"
    ))?;
    let rows = stmt
        .query_map(params![account_id, thread_id], |r| Ok((row_to_summary(r, 1, 0)?, r.get::<_, String>(21)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out: Vec<(MessageSummary, String)> = Vec::new();
    for (m, role) in rows {
        if let Some(mid) = &m.message_id {
            if let Some(existing) = out.iter_mut().find(|(e, _)| e.message_id.as_deref() == Some(mid)) {
                // Prefer the copy that is not in "All Mail".
                if existing.1 == "all" && role != "all" {
                    *existing = (m, role);
                }
                continue;
            }
        }
        out.push((m, role));
    }
    let n = out.len() as i64;
    let unread = out.iter().filter(|(m, _)| !m.flags.seen).count() as i64;
    Ok(out
        .into_iter()
        .map(|(mut m, _)| {
            m.thread_count = n;
            m.thread_unread = unread;
            m
        })
        .collect())
}

/// (folder_id, folder path, uid, account_id) for a message rowid.
pub fn locate(conn: &Connection, id: i64) -> Result<(i64, String, u32, String)> {
    conn.query_row(
        "SELECT m.folder_id, f.path, m.uid, m.account_id FROM messages m JOIN folders f ON f.id=m.folder_id WHERE m.id=?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u32, r.get(3)?)),
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => ApiError::not_found("The message"),
        other => other.into(),
    })
}

pub fn body_raw(conn: &Connection, id: i64) -> Result<Option<Vec<u8>>> {
    Ok(conn.query_row("SELECT raw FROM bodies WHERE message_rowid=?1", params![id], |r| r.get(0)).optional()?)
}

pub fn assemble_full(conn: &Connection, id: i64, raw: &[u8], allow_remote: bool) -> Result<MessageFull> {
    let summary = get_summary(conn, id)?;
    let (bcc, reply_to, irt, refs, lu, auth): (String, String, Option<String>, String, Option<String>, Option<String>) = conn.query_row(
        "SELECT bcc_json, reply_to_json, in_reply_to, refs, list_unsub_json, auth_json FROM messages WHERE id=?1",
        params![id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
    )?;
    let parsed = mime::parse_body(raw, allow_remote);
    Ok(MessageFull {
        summary: MessageSummary { snippet: if summary.snippet.is_empty() { parsed.snippet.clone() } else { summary.snippet.clone() }, ..summary },
        bcc: serde_json::from_str(&bcc).unwrap_or_default(),
        reply_to: serde_json::from_str(&reply_to).unwrap_or_default(),
        in_reply_to: irt,
        references: serde_json::from_str(&refs).unwrap_or_default(),
        html: parsed.html,
        text: parsed.text,
        plain_text: parsed.plain_text,
        remote_content_blocked: parsed.remote_content_blocked,
        trackers_removed: parsed.trackers_removed,
        attachments: parsed.attachments.into_iter().filter(|a| !a.inline).collect(),
        list_unsubscribe: lu.and_then(|s| serde_json::from_str(&s).ok()),
        auth_results: auth.and_then(|s| serde_json::from_str(&s).ok()),
        invite: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Store;

    pub fn seed(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO accounts(id,name,email,color,provider,imap_json,smtp_json) VALUES ('a','A','a@x','#c41e1e','imap','{}','{}');
             INSERT INTO folders(id,account_id,path,name,role) VALUES (1,'a','INBOX','Inbox','inbox'), (2,'a','Trash','Trash','trash');",
        )
        .unwrap();
    }

    fn hdr(mid: &str, irt: Option<&str>, subj: &str, date: i64) -> HeaderInfo {
        HeaderInfo {
            subject: subj.into(),
            from: Address { name: Some("Ann".into()), email: "ann@example.com".into() },
            message_id: Some(mid.into()),
            in_reply_to: irt.map(|s| s.into()),
            references: irt.map(|s| vec![s.to_string()]).unwrap_or_default(),
            date: Some(date),
            ..Default::default()
        }
    }

    #[test]
    fn threaded_view_counts() {
        let s = Store::open_in_memory().unwrap();
        s.with(|c| {
            seed(c);
            let mk = |uid, h: HeaderInfo, seen| NewMessage {
                account_id: "a",
                folder_id: 1,
                uid,
                header: h,
                flags: ParsedFlags { flags: Flags { seen, ..Default::default() }, ..Default::default() },
                size: 100,
                internal_date: None,
                has_attachments: false,
                modseq: None,
                gm_thrid: None,
                gm_msgid: None,
                ext_thread: None,
            };
            insert_message(c, &mk(1, hdr("r@x", None, "Plan", 1000), true))?;
            insert_message(c, &mk(2, hdr("c@x", Some("r@x"), "Re: Plan", 2000), false))?;
            insert_message(c, &mk(3, hdr("o@x", None, "Other", 1500), false))?;
            let q = ViewQuery { folder_id: Some("1".into()), unified: None, filter: None, search: None, threaded: true, sort: SortOrder::DateDesc, offset: 0, limit: 50 };
            let page = list_view(c, &q)?;
            assert_eq!(page.total, 2);
            assert_eq!(page.items[0].subject, "Re: Plan");
            assert_eq!(page.items[0].thread_count, 2);
            assert_eq!(page.items[0].thread_unread, 1);
            let flat = list_view(c, &ViewQuery { threaded: false, ..q.clone() })?;
            assert_eq!(flat.total, 3);
            let unified = list_view(c, &ViewQuery { folder_id: None, unified: Some(UnifiedView::Inbox), threaded: false, ..q.clone() })?;
            assert_eq!(unified.total, 3);
            let unread = list_view(c, &ViewQuery { filter: Some(ViewFilter { unread: Some(true), ..Default::default() }), threaded: false, ..q.clone() })?;
            assert_eq!(unread.total, 2);
            let searched = list_view(c, &ViewQuery { search: Some("other".into()), threaded: false, ..q.clone() })?;
            assert_eq!(searched.total, 1);
            let t = thread_messages(c, "a", &page.items[0].thread_id)?;
            assert_eq!(t.len(), 2);
            assert_eq!(t[0].subject, "Plan");
            // Flags + delete.
            update_flags(c, 1, 2, &ParsedFlags { flags: Flags { seen: true, ..Default::default() }, ..Default::default() }, None)?;
            let unread = list_view(c, &ViewQuery { filter: Some(ViewFilter { unread: Some(true), ..Default::default() }), threaded: false, ..q.clone() })?;
            assert_eq!(unread.total, 1);
            delete_uid(c, 1, 3)?;
            let flat = list_view(c, &ViewQuery { threaded: false, ..q })?;
            assert_eq!(flat.total, 2);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn flag_parsing() {
        let f = parse_flags(&["\\Seen".into(), "$Forwarded".into(), "$label1".into(), "NonJunk".into(), "\\Recent".into()]);
        assert!(f.flags.seen && f.flags.forwarded);
        assert_eq!(f.keywords, vec!["$label1"]);
    }
}
