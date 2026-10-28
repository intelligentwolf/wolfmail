//! Local-store operations shared by the Gmail and Graph adapters. Synchronous: callers run them inside
//! `Store::call` (blocking pool), so no lock is ever held across an `.await`.
// Source: db.rs migration v2 (remote_id, sync_gen, message_labels, folders.sync_state, accounts.api_state).
use crate::error::Result;
use crate::mime::{self, HeaderInfo};
use crate::model::*;
use crate::msgstore::{self, NewMessage, ParsedFlags};
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::Serialize;

pub struct ApiFolder<'a> {
    pub remote_id: &'a str,
    pub path: &'a str,
    pub name: &'a str,
    pub role: Option<FolderRole>,
    pub parent_path: Option<&'a str>,
    pub depth: i64,
    pub selectable: bool,
}

/// Insert or update a folder keyed by its remote id (falling back to its path). Returns the local id.
pub fn upsert_folder(c: &Connection, account_id: &str, f: &ApiFolder<'_>) -> Result<i64> {
    let existing: Option<i64> = c
        .query_row("SELECT id FROM folders WHERE account_id=?1 AND remote_id=?2", params![account_id, f.remote_id], |r| r.get(0))
        .optional()?;
    let id = match existing {
        Some(id) => {
            // The path of another (stale) folder may collide after a rename elsewhere; move it out of the way.
            c.execute(
                "UPDATE folders SET path = path || char(1) || id WHERE account_id=?1 AND path=?2 AND id<>?3",
                params![account_id, f.path, id],
            )?;
            c.execute(
                "UPDATE folders SET path=?2, name=?3, role=?4, selectable=?5, depth=?6, delimiter='/' WHERE id=?1",
                params![id, f.path, f.name, f.role.map(|r| r.as_str()), f.selectable, f.depth],
            )?;
            id
        }
        None => {
            c.execute(
                "INSERT INTO folders(account_id, path, name, role, delimiter, selectable, depth, remote_id) VALUES (?1,?2,?3,?4,'/',?5,?6,?7)
                 ON CONFLICT(account_id, path) DO UPDATE SET name=excluded.name, role=excluded.role, selectable=excluded.selectable,
                    depth=excluded.depth, remote_id=excluded.remote_id, delimiter='/'",
                params![account_id, f.path, f.name, f.role.map(|r| r.as_str()), f.selectable, f.depth, f.remote_id],
            )?;
            c.query_row("SELECT id FROM folders WHERE account_id=?1 AND path=?2", params![account_id, f.path], |r| r.get(0))?
        }
    };
    let parent: Option<i64> = match f.parent_path {
        Some(p) => c.query_row("SELECT id FROM folders WHERE account_id=?1 AND path=?2", params![account_id, p], |r| r.get(0)).optional()?,
        None => None,
    };
    c.execute("UPDATE folders SET parent_id=?2 WHERE id=?1", params![id, parent])?;
    Ok(id)
}

/// Remove folders of the account whose remote id is not in `keep` (deleted on the server). `listed_at` is when the
/// list request was sent: folders created locally after it are not in `keep` yet and are left alone.
pub fn remove_folders_except(c: &Connection, account_id: &str, keep: &[String], listed_at: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64, Option<String>)> = {
        let mut st = c.prepare("SELECT id, remote_id FROM folders WHERE account_id=?1 AND created_at < ?2")?;
        let v = st.query_map(params![account_id, listed_at], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let mut removed = Vec::new();
    for (id, rid) in rows {
        if rid.as_ref().is_none_or(|r| !keep.contains(r)) {
            c.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE folder_id=?1)", params![id])?;
            c.execute("DELETE FROM folders WHERE id=?1", params![id])?;
            removed.push(id);
        }
    }
    Ok(removed)
}

pub fn folder_by_remote(c: &Connection, account_id: &str, remote_id: &str) -> Result<Option<i64>> {
    Ok(c.query_row("SELECT id FROM folders WHERE account_id=?1 AND remote_id=?2", params![account_id, remote_id], |r| r.get(0)).optional()?)
}

pub fn folder_remote(c: &Connection, folder_id: i64) -> Result<(String, Option<String>, Option<FolderRole>, String)> {
    Ok(c.query_row("SELECT account_id, remote_id, role, path FROM folders WHERE id=?1", params![folder_id], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.as_deref().and_then(FolderRole::parse), r.get(3)?))
    })?)
}

/// Folder ids of the account, by role.
pub fn role_folder(c: &Connection, account_id: &str, role: FolderRole) -> Result<Option<(i64, String)>> {
    Ok(c.query_row(
        "SELECT id, COALESCE(remote_id,'') FROM folders WHERE account_id=?1 AND role=?2",
        params![account_id, role.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?)
}

/// Everything about one message the adapters need to act on it.
#[derive(Debug, Clone)]
pub struct RemoteRow {
    pub rowid: i64,
    pub account_id: String,
    pub remote_id: String,
    pub folder_id: i64,
    pub folder_remote: Option<String>,
    pub folder_role: Option<FolderRole>,
    pub thread_id: String,
    pub seen: bool,
    pub flagged: bool,
    pub keywords: Vec<String>,
    /// Gmail label folders (local id, remote label id, role).
    pub labels: Vec<(i64, String, Option<FolderRole>)>,
}

pub fn remote_row(c: &Connection, rowid: i64) -> Result<RemoteRow> {
    let mut row = c
        .query_row(
            "SELECT m.account_id, m.remote_id, m.folder_id, f.remote_id, f.role, m.thread_id, m.seen, m.flagged, m.keywords
             FROM messages m JOIN folders f ON f.id=m.folder_id WHERE m.id=?1",
            params![rowid],
            |r| {
                let kw: String = r.get(8)?;
                Ok(RemoteRow {
                    rowid,
                    account_id: r.get(0)?,
                    remote_id: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    folder_id: r.get(2)?,
                    folder_remote: r.get(3)?,
                    folder_role: r.get::<_, Option<String>>(4)?.as_deref().and_then(FolderRole::parse),
                    thread_id: r.get(5)?,
                    seen: r.get(6)?,
                    flagged: r.get(7)?,
                    keywords: serde_json::from_str(&kw).unwrap_or_default(),
                    labels: Vec::new(),
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => crate::error::ApiError::not_found("The message"),
            o => o.into(),
        })?;
    if row.remote_id.is_empty() {
        return Err(crate::error::ApiError::internal("This message has no server id yet; wait for the next sync."));
    }
    let mut st = c.prepare(
        "SELECT f.id, COALESCE(f.remote_id,''), f.role FROM message_labels ml JOIN folders f ON f.id=ml.folder_id WHERE ml.message_rowid=?1",
    )?;
    row.labels = st
        .query_map(params![rowid], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.as_deref().and_then(FolderRole::parse))))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(row)
}

pub fn row_by_remote(c: &Connection, account_id: &str, remote_id: &str) -> Result<Option<(i64, i64)>> {
    Ok(c.query_row("SELECT id, folder_id FROM messages WHERE account_id=?1 AND remote_id=?2", params![account_id, remote_id], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })
    .optional()?)
}

/// A message as listed by a provider, ready to store.
pub struct ApiMessage {
    pub remote_id: String,
    pub folder_id: i64,
    pub header: HeaderInfo,
    pub flags: Flags,
    pub keywords: Vec<String>,
    pub size: i64,
    pub internal_date: Option<i64>,
    pub has_attachments: bool,
    pub ext_thread: Option<String>,
    pub snippet: String,
    /// Gmail label folder ids (empty for Graph).
    pub labels: Vec<i64>,
}

/// Insert a new message or update the stored one (same remote id): flags, tags, folder, labels and generation.
/// Header fields of an existing row are left alone — they never change for a delivered message. Returns
/// (rowid, newly inserted).
pub fn upsert_message(c: &Connection, account_id: &str, m: ApiMessage, gen: i64) -> Result<(i64, bool)> {
    if let Some((id, folder)) = row_by_remote(c, account_id, &m.remote_id)? {
        if folder != m.folder_id {
            let uid = next_uid(c, m.folder_id)?;
            c.execute("UPDATE messages SET folder_id=?2, uid=?3 WHERE id=?1", params![id, m.folder_id, uid])?;
        }
        c.execute(
            "UPDATE messages SET seen=?2, flagged=?3, draft=?4, keywords=?5, sync_gen=MAX(sync_gen, ?6),
                snippet=CASE WHEN body_cached=1 THEN snippet ELSE ?7 END WHERE id=?1",
            params![id, m.flags.seen, m.flags.flagged, m.flags.draft, serde_json::to_string(&m.keywords)?, gen, m.snippet],
        )?;
        set_labels(c, id, &m.labels)?;
        return Ok((id, false));
    }
    let uid = next_uid(c, m.folder_id)?;
    let id = msgstore::insert_message(
        c,
        &NewMessage {
            account_id,
            folder_id: m.folder_id,
            uid,
            header: m.header,
            flags: ParsedFlags { flags: m.flags, deleted: false, keywords: m.keywords },
            size: m.size,
            internal_date: m.internal_date,
            has_attachments: m.has_attachments,
            modseq: None,
            gm_thrid: None,
            gm_msgid: None,
            ext_thread: m.ext_thread,
        },
    )?;
    c.execute("UPDATE messages SET remote_id=?2, sync_gen=?3, snippet=?4 WHERE id=?1", params![id, m.remote_id, gen, m.snippet])?;
    set_labels(c, id, &m.labels)?;
    Ok((id, true))
}

/// Synthetic per-folder UID for API rows: the (folder_id, uid) uniqueness of the shared schema still holds.
fn next_uid(c: &Connection, folder_id: i64) -> Result<u32> {
    let max: i64 = c.query_row("SELECT COALESCE(MAX(uid),0) FROM messages WHERE folder_id=?1", params![folder_id], |r| r.get(0))?;
    Ok((max + 1).min(u32::MAX as i64) as u32)
}

pub fn set_labels(c: &Connection, rowid: i64, folders: &[i64]) -> Result<()> {
    c.execute("DELETE FROM message_labels WHERE message_rowid=?1", params![rowid])?;
    for f in folders {
        c.execute("INSERT OR IGNORE INTO message_labels(message_rowid, folder_id) VALUES (?1, ?2)", params![rowid, f])?;
    }
    Ok(())
}

/// Delete by remote id; with `only_folder`, only when the row is (still) in that folder — a Graph move reports
/// "removed" in the source folder, and the destination folder may already have claimed the row.
pub fn delete_remote(c: &Connection, account_id: &str, remote_id: &str, only_folder: Option<i64>) -> Result<bool> {
    if let Some((id, folder)) = row_by_remote(c, account_id, remote_id)? {
        if only_folder.is_none_or(|f| f == folder) {
            msgstore::delete_row(c, id)?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// After a completed full listing at generation `gen`: rows of the scope not seen in it were deleted on the
/// server. Scope = one folder (Graph) or the whole account (Gmail).
pub fn sweep_unseen(c: &Connection, account_id: &str, folder_id: Option<i64>, gen: i64) -> Result<usize> {
    let ids: Vec<i64> = {
        let mut st = c.prepare(
            "SELECT id FROM messages WHERE account_id=?1 AND remote_id IS NOT NULL AND sync_gen < ?2 AND (?3 IS NULL OR folder_id=?3)",
        )?;
        let v = st.query_map(params![account_id, gen, folder_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    for id in &ids {
        msgstore::delete_row(c, *id)?;
    }
    Ok(ids.len())
}

/// Store a fetched raw body and refresh the header columns the list API could not give us (Graph lists have no
/// References / In-Reply-To / List-Unsubscribe / Authentication-Results).
pub fn store_body_and_headers(c: &Connection, rowid: i64, raw: &[u8]) -> Result<()> {
    msgstore::store_body(c, rowid, raw)?;
    let h = mime::parse_headers(raw);
    c.execute(
        "UPDATE messages SET in_reply_to=COALESCE(?2, in_reply_to), refs=CASE WHEN ?3='[]' THEN refs ELSE ?3 END,
            list_unsub_json=COALESCE(?4, list_unsub_json), auth_json=COALESCE(?5, auth_json),
            bcc_json=CASE WHEN ?6='[]' THEN bcc_json ELSE ?6 END WHERE id=?1",
        params![
            rowid,
            h.in_reply_to,
            serde_json::to_string(&h.references)?,
            h.list_unsubscribe.as_ref().map(serde_json::to_string).transpose()?,
            h.auth_results.as_ref().map(serde_json::to_string).transpose()?,
            serde_json::to_string(&h.bcc)?,
        ],
    )?;
    Ok(())
}

/// Rows of the account (optionally one folder) without a cached body, newer than `since`, newest first.
pub fn bodies_wanted(c: &Connection, account_id: &str, since: i64, limit: i64) -> Result<Vec<(i64, String)>> {
    let mut st = c.prepare(
        "SELECT id, remote_id FROM messages WHERE account_id=?1 AND remote_id IS NOT NULL AND body_cached=0 AND date>=?2
           AND size < 26214400 ORDER BY date DESC LIMIT ?3",
    )?;
    let v = st.query_map(params![account_id, since, limit], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn account_state<T: DeserializeOwned + Default>(c: &Connection, account_id: &str) -> Result<T> {
    let raw: Option<String> = c.query_row("SELECT api_state FROM accounts WHERE id=?1", params![account_id], |r| r.get(0)).optional()?.flatten();
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

pub fn set_account_state<T: Serialize>(c: &Connection, account_id: &str, st: &T) -> Result<()> {
    c.execute("UPDATE accounts SET api_state=?2 WHERE id=?1", params![account_id, serde_json::to_string(st)?])?;
    Ok(())
}

pub fn folder_state<T: DeserializeOwned + Default>(c: &Connection, folder_id: i64) -> Result<T> {
    let raw: Option<String> = c.query_row("SELECT sync_state FROM folders WHERE id=?1", params![folder_id], |r| r.get(0)).optional()?.flatten();
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

pub fn set_folder_state<T: Serialize>(c: &Connection, folder_id: i64, st: &T) -> Result<()> {
    c.execute("UPDATE folders SET sync_state=?2 WHERE id=?1", params![folder_id, serde_json::to_string(st)?])?;
    Ok(())
}

/// Map provider tag names (Gmail user-label names / Outlook category names) to WolfMail tag keys; names that are
/// not a tag yet become one, so a category set in Outlook shows up as a tag here.
pub fn tag_keys_for_names(c: &Connection, names: &[String], create_missing: bool) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for n in names {
        let key: Option<String> = c.query_row("SELECT key FROM tags WHERE lower(name)=lower(?1)", params![n], |r| r.get(0)).optional()?;
        match key {
            Some(k) => out.push(k),
            None if create_missing => {
                let slug: String = n.to_lowercase().chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' }).collect();
                let k = format!("cat_{slug}");
                // Source: accounts.rs PALETTE[2] — a neutral IntelligentWolf chart colour for tags we did not choose.
                c.execute(
                    "INSERT INTO tags(key, name, color, sort_order) VALUES (?1, ?2, '#7a6a86', (SELECT COALESCE(MAX(sort_order),0)+1 FROM tags))
                     ON CONFLICT(key) DO NOTHING",
                    params![k, n],
                )?;
                out.push(k);
            }
            None => {}
        }
    }
    Ok(out)
}

/// Tag display name for a key (the provider stores names, WolfMail stores keys).
pub fn tag_name(c: &Connection, key: &str) -> Result<Option<String>> {
    Ok(c.query_row("SELECT name FROM tags WHERE key=?1", params![key], |r| r.get(0)).optional()?)
}

/// Build an RFC 5322 header block from (name, value) pairs so the shared parser reads API metadata exactly the
/// way it reads IMAP headers.
pub fn header_block(headers: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (n, v) in headers {
        // Values from the API are already unfolded; strip CR/LF defensively so one value cannot inject another.
        let v: String = v.chars().filter(|c| *c != '\r' && *c != '\n').collect();
        out.extend_from_slice(n.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(v.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
    out
}
