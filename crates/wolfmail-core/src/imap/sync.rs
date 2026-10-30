//! Folder list + per-folder synchronisation.
//!
//! Source of the algorithm: RFC 4549 §4 ("Synchronization Operations for Disconnected IMAP4 Clients") for the
//! UID-range + flag-resync path, RFC 7162 §3.1 (CONDSTORE CHANGEDSINCE) and §3.2.5 (QRESYNC SELECT with VANISHED)
//! for the fast path. Headers first; bodies prefetched only for messages newer than the account's `bodyDays`.
use super::conn::Conn;
use super::proto::{uid_set, FetchRow};
use crate::db::{now_ms, Store};
use crate::error::Result;
use crate::mime;
use crate::model::FolderRole;
use crate::msgstore::{self, NewMessage};
use crate::utf7;
use rusqlite::{params, OptionalExtension};
use std::collections::HashSet;

/// Header fields fetched for the list view. Small and enough for threading, unsubscribe and auth badges.
const HEADER_FIELDS: &str = "FROM TO CC BCC REPLY-TO SUBJECT DATE MESSAGE-ID IN-REPLY-TO REFERENCES LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST AUTHENTICATION-RESULTS CONTENT-TYPE LIST-ID PRECEDENCE AUTO-SUBMITTED FEEDBACK-ID";
const NEW_CHUNK: u32 = 500;
const BACKFILL_PER_PASS: u32 = 2000;
const FULL_FLAG_SCAN_LIMIT: i64 = 20_000;
const PARTIAL_FLAG_SCAN: i64 = 5_000;
const BODY_BATCH: usize = 25;
const BODY_PER_PASS: i64 = 200;

#[derive(Debug, Clone)]
pub struct AccountCtx {
    pub account_id: String,
    pub body_days: i64,
}

#[derive(Debug, Clone)]
pub struct FolderRef {
    pub id: i64,
    pub path: String,
    pub role: Option<FolderRole>,
}

#[derive(Debug, Default)]
pub struct SyncOutcome {
    pub changed: bool,
    pub new_ids: Vec<i64>,
    pub more_backfill: bool,
}

fn header_items(conn: &Conn) -> String {
    let mut s = format!("(UID FLAGS INTERNALDATE RFC822.SIZE BODYSTRUCTURE BODY.PEEK[HEADER.FIELDS ({HEADER_FIELDS})]");
    if conn.condstore() {
        s.push_str(" MODSEQ");
    }
    if conn.is_gmail() {
        s.push_str(" X-GM-THRID X-GM-MSGID");
    }
    s.push(')');
    s
}

/// Name heuristics used when the server does not advertise SPECIAL-USE (RFC 6154). Lower-case, last segment.
pub fn role_from_name(path: &str, last: &str) -> Option<FolderRole> {
    if path.eq_ignore_ascii_case("INBOX") {
        return Some(FolderRole::Inbox);
    }
    let l = last.to_lowercase();
    Some(match l.as_str() {
        "sent" | "sent items" | "sent messages" | "sent mail" | "gesendet" | "gesendete elemente" | "envoyés" | "éléments envoyés" | "enviados" | "inviata" | "posta inviata" | "verzonden" => FolderRole::Sent,
        "drafts" | "draft" | "entwürfe" | "brouillons" | "borradores" | "bozze" | "concepten" => FolderRole::Drafts,
        "trash" | "deleted items" | "deleted messages" | "deleted" | "bin" | "papierkorb" | "corbeille" | "papelera" | "cestino" | "prullenbak" => FolderRole::Trash,
        "junk" | "spam" | "junk e-mail" | "junk email" | "bulk mail" | "courrier indésirable" => FolderRole::Junk,
        "archive" | "archives" | "archiv" => FolderRole::Archive,
        "all mail" => FolderRole::All,
        "starred" | "flagged" => FolderRole::Flagged,
        _ => return None,
    })
}

pub async fn sync_folder_list(conn: &mut Conn, store: &Store, account_id: &str) -> Result<Vec<FolderRef>> {
    // Taken before LIST: folders created locally after this moment are not in the answer and must not be removed.
    let listed_at = now_ms();
    let entries = conn.list_all().await?;
    let account_id = account_id.to_string();
    store
        .call(move |c| {
            let tx = c.transaction()?;
            let mut seen_paths = HashSet::new();
            let mut used_roles: HashSet<FolderRole> = HashSet::new();
            // Special-use first so it wins over a name match on another folder.
            let mut ordered = entries.clone();
            ordered.sort_by_key(|e| (e.special.is_none(), !e.path.eq_ignore_ascii_case("INBOX"), e.path.clone()));
            for e in &ordered {
                let delim = e.delimiter.clone().unwrap_or_default();
                let segments: Vec<&str> = if delim.is_empty() { vec![e.path.as_str()] } else { e.path.split(delim.as_str()).collect() };
                let last_raw = segments.last().copied().unwrap_or(&e.path);
                let name = if e.path.eq_ignore_ascii_case("INBOX") { "Inbox".to_string() } else { utf7::decode(last_raw) };
                let mut role = e.special.or_else(|| role_from_name(&e.path, &utf7::decode(last_raw)));
                if let Some(r) = role {
                    if used_roles.contains(&r) {
                        role = None;
                    } else {
                        used_roles.insert(r);
                    }
                }
                let depth = segments.len().saturating_sub(1) as i64;
                tx.execute(
                    "INSERT INTO folders(account_id, path, name, role, delimiter, selectable, depth) VALUES (?1,?2,?3,?4,?5,?6,?7)
                     ON CONFLICT(account_id, path) DO UPDATE SET name=excluded.name, role=excluded.role, delimiter=excluded.delimiter,
                        selectable=excluded.selectable, depth=excluded.depth",
                    params![account_id, e.path, name, role.map(|r| r.as_str()), e.delimiter, e.selectable, depth],
                )?;
                seen_paths.insert(e.path.clone());
            }
            // Parents.
            for e in &ordered {
                if let Some(d) = e.delimiter.as_deref().filter(|d| !d.is_empty()) {
                    if let Some((parent, _)) = e.path.rsplit_once(d) {
                        tx.execute(
                            "UPDATE folders SET parent_id=(SELECT id FROM folders WHERE account_id=?1 AND path=?2) WHERE account_id=?1 AND path=?3",
                            params![account_id, parent, e.path],
                        )?;
                    }
                }
            }
            // Folders gone from the server (deleted elsewhere) — but not ones created after LIST was sent.
            let existing: Vec<(i64, String)> = {
                let mut st = tx.prepare("SELECT id, path FROM folders WHERE account_id=?1 AND created_at < ?2")?;
                let rows = st.query_map(params![account_id, listed_at], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (id, path) in existing {
                if !seen_paths.contains(&path) {
                    tx.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE folder_id=?1)", params![id])?;
                    tx.execute("DELETE FROM folders WHERE id=?1", params![id])?;
                }
            }
            tx.commit()?;
            let mut st = c.prepare("SELECT id, path, role FROM folders WHERE account_id=?1 AND selectable=1 ORDER BY (role='inbox') DESC, depth, path")?;
            let rows = st
                .query_map(params![account_id], |r| {
                    Ok(FolderRef { id: r.get(0)?, path: r.get(1)?, role: r.get::<_, Option<String>>(2)?.as_deref().and_then(FolderRole::parse) })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await
}

/// Source: RFC 3501 §9 date-time = DQUOTE date-day-fixed "-" date-month "-" date-year SP time SP zone DQUOTE.
pub fn parse_internal_date(s: &str) -> Option<i64> {
    let t = s.trim().trim_matches('"').trim();
    // date-day-fixed may be space-padded (" 7-Jul-1996"); chrono's %d wants two digits.
    let t = if t.as_bytes().get(1) == Some(&b'-') { format!("0{t}") } else { t.to_string() };
    chrono::DateTime::parse_from_str(&t, "%d-%b-%Y %H:%M:%S %z").ok().map(|d| d.timestamp_millis())
}

struct FolderState {
    uidvalidity: Option<u32>,
    highest_modseq: Option<u64>,
    max_uid: Option<u32>,
    min_uid: Option<u32>,
    local_count: i64,
    /// UIDNEXT seen at the end of the last completed sync of this folder (None = never synced, or reset with
    /// UIDVALIDITY). Anything at or above it arrived after that sync.
    uidnext: Option<u32>,
}

impl FolderState {
    /// Highest UID already accounted for. Uses the remembered UIDNEXT as well as the highest cached row, because a
    /// folder emptied by archiving everything has no rows left — without UIDNEXT its next message would look like a
    /// first sync and never be announced or filtered (found by tests/rules_greenmail.rs, 28 Oct 2026).
    fn watermark(&self) -> Option<u32> {
        match (self.max_uid, self.uidnext) {
            (None, None) => None,
            (m, n) => Some(m.unwrap_or(0).max(n.map(|n| n.saturating_sub(1)).unwrap_or(0))),
        }
    }
}

async fn load_state(store: &Store, folder_id: i64) -> Result<FolderState> {
    store
        .call(move |c| {
            let (uv, ms, un): (Option<i64>, Option<i64>, Option<i64>) = c.query_row(
                "SELECT uidvalidity, highest_modseq, uidnext FROM folders WHERE id=?1",
                params![folder_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            let (mx, mn, n): (Option<i64>, Option<i64>, i64) =
                c.query_row("SELECT MAX(uid), MIN(uid), COUNT(*) FROM messages WHERE folder_id=?1", params![folder_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(FolderState {
                uidvalidity: uv.map(|v| v as u32),
                highest_modseq: ms.map(|v| v as u64),
                max_uid: mx.map(|v| v as u32),
                min_uid: mn.map(|v| v as u32),
                local_count: n,
                uidnext: un.map(|v| v as u32),
            })
        })
        .await
}

/// Rows saved per write transaction, so a big fetch never holds the database for long (db.rs Store::read).
const WRITE_CHUNK: usize = 50;

async fn store_rows(store: &Store, ctx: &AccountCtx, folder_id: i64, rows: Vec<FetchRow>, min_uid_exclusive: Option<u32>) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    let mut rest = rows;
    while !rest.is_empty() {
        let tail = rest.split_off(rest.len().min(WRITE_CHUNK));
        let chunk = std::mem::replace(&mut rest, tail);
        ids.extend(store_row_chunk(store, ctx, folder_id, chunk, min_uid_exclusive).await?);
    }
    Ok(ids)
}

async fn store_row_chunk(store: &Store, ctx: &AccountCtx, folder_id: i64, rows: Vec<FetchRow>, min_uid_exclusive: Option<u32>) -> Result<Vec<i64>> {
    let account_id = ctx.account_id.clone();
    store
        .call(move |c| {
            let tx = c.transaction()?;
            let mut ids = Vec::new();
            for r in rows {
                let Some(uid) = r.uid else { continue };
                if min_uid_exclusive.is_some_and(|m| uid <= m) {
                    continue;
                }
                let header = r.header.as_deref().map(mime::parse_headers).unwrap_or_default();
                let flags = msgstore::parse_flags(&r.flags.clone().unwrap_or_default());
                if flags.deleted {
                    continue;
                }
                let id = msgstore::insert_message(
                    &tx,
                    &NewMessage {
                        account_id: &account_id,
                        folder_id,
                        uid,
                        header,
                        flags,
                        size: r.size.unwrap_or(0) as i64,
                        internal_date: r.internal_date.as_deref().and_then(parse_internal_date),
                        has_attachments: r.has_attachments.unwrap_or(false),
                        modseq: r.modseq,
                        gm_thrid: r.gm_thrid,
                        gm_msgid: r.gm_msgid,
                        ext_thread: None,
                    },
                )?;
                ids.push(id);
            }
            tx.commit()?;
            Ok(ids)
        })
        .await
}

async fn apply_flag_rows(store: &Store, folder_id: i64, rows: Vec<FetchRow>) -> Result<bool> {
    if rows.is_empty() {
        return Ok(false);
    }
    store
        .call(move |c| {
            let tx = c.transaction()?;
            let mut changed = false;
            for r in rows {
                let (Some(uid), Some(flags)) = (r.uid, r.flags.as_ref()) else { continue };
                let parsed = msgstore::parse_flags(flags);
                let before: Option<(bool, bool, bool, bool, String)> = tx
                    .query_row(
                        "SELECT seen, flagged, answered, forwarded, keywords FROM messages WHERE folder_id=?1 AND uid=?2",
                        params![folder_id, uid],
                        |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?, x.get(3)?, x.get(4)?)),
                    )
                    .optional()?;
                let Some(before) = before else { continue };
                let kw = serde_json::to_string(&parsed.keywords)?;
                if parsed.deleted
                    || before != (parsed.flags.seen, parsed.flags.flagged, parsed.flags.answered, parsed.flags.forwarded, kw)
                {
                    changed = true;
                }
                msgstore::update_flags(&tx, folder_id, uid, &parsed, r.modseq)?;
            }
            tx.commit()?;
            Ok(changed)
        })
        .await
}

async fn delete_uids(store: &Store, folder_id: i64, uids: Vec<u32>) -> Result<bool> {
    if uids.is_empty() {
        return Ok(false);
    }
    store
        .call(move |c| {
            let tx = c.transaction()?;
            for u in uids {
                msgstore::delete_uid(&tx, folder_id, u)?;
            }
            tx.commit()?;
            Ok(true)
        })
        .await
}

pub async fn sync_folder(conn: &mut Conn, store: &Store, ctx: &AccountCtx, folder: &FolderRef) -> Result<SyncOutcome> {
    let mut out = SyncOutcome::default();
    let mut st = load_state(store, folder.id).await?;
    let known = match (st.uidvalidity, st.highest_modseq) {
        (Some(uv), Some(ms)) if st.local_count > 0 => Some((uv, ms)),
        _ => None,
    };
    let sel = conn.select(&folder.path, known).await?;

    // UIDVALIDITY changed: every cached UID is meaningless (RFC 3501 §2.3.1.1).
    if let (Some(old), Some(new)) = (st.uidvalidity, sel.uidvalidity) {
        if old != new {
            let fid = folder.id;
            store
                .call(move |c| {
                    c.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE folder_id=?1)", params![fid])?;
                    c.execute("DELETE FROM messages WHERE folder_id=?1", params![fid])?;
                    c.execute("UPDATE folders SET highest_modseq=NULL, uidnext=NULL WHERE id=?1", params![fid])?;
                    Ok(())
                })
                .await?;
            out.changed = true;
            st = load_state(store, folder.id).await?;
        }
    }
    let qresync_used = conn.qresync && known.is_some() && st.local_count > 0;

    // 1. Changes to messages we already have.
    if st.local_count > 0 {
        if qresync_used {
            let vanished: Vec<u32> = sel.vanished.iter().flat_map(|r| r.clone()).collect();
            out.changed |= delete_uids(store, folder.id, vanished).await?;
            out.changed |= apply_flag_rows(store, folder.id, sel.changed.clone()).await?;
        } else if conn.condstore() && !sel.nomodseq && st.highest_modseq.is_some() {
            let ms = st.highest_modseq.unwrap_or(0);
            if sel.highest_modseq != Some(ms) {
                let lo = st.min_uid.unwrap_or(1);
                let (rows, _) = conn.uid_fetch(&format!("{lo}:*"), "(UID FLAGS)", Some(&format!("(CHANGEDSINCE {ms})"))).await?;
                out.changed |= apply_flag_rows(store, folder.id, rows).await?;
            }
        } else {
            // No mod-sequences: rescan flags (bounded for very large folders).
            let lo = if st.local_count > FULL_FLAG_SCAN_LIMIT {
                let fid = folder.id;
                store
                    .call(move |c| {
                        Ok(c.query_row(
                            "SELECT MIN(uid) FROM (SELECT uid FROM messages WHERE folder_id=?1 ORDER BY uid DESC LIMIT ?2)",
                            params![fid, PARTIAL_FLAG_SCAN],
                            |r| r.get::<_, Option<i64>>(0),
                        )?)
                    })
                    .await?
                    .unwrap_or(1) as u32
            } else {
                st.min_uid.unwrap_or(1)
            };
            let (rows, _) = conn.uid_fetch(&format!("{lo}:*"), "(UID FLAGS)", None).await?;
            out.changed |= apply_flag_rows(store, folder.id, rows).await?;
        }
        if !qresync_used {
            // Expunges: compare the UIDs the server still has in our cached range with what we hold.
            let lo = st.min_uid.unwrap_or(1);
            let resps = conn.raw(&format!("UID SEARCH UID {lo}:*")).await?;
            let server: HashSet<u32> = super::proto::parse_search(&resps).into_iter().collect();
            let fid = folder.id;
            let local: Vec<u32> = store
                .call(move |c| {
                    let mut s = c.prepare("SELECT uid FROM messages WHERE folder_id=?1")?;
                    let v = s.query_map(params![fid], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(v.into_iter().map(|u| u as u32).collect())
                })
                .await?;
            let gone: Vec<u32> = local.into_iter().filter(|u| !server.contains(u)).collect();
            out.changed |= delete_uids(store, folder.id, gone).await?;
        }
    }

    // 2. New messages above our highest UID (or, on first sync, the newest NEW_CHUNK by sequence number).
    let items = header_items(conn);
    if let Some(max) = st.watermark() {
        if sel.uidnext.is_none_or(|n| n > max + 1) {
            // "n:*" always returns at least the highest message even when its UID < n (RFC 3501 §6.4.8), so rows at
            // or below `max` are skipped in store_rows.
            let (rows, _) = conn.uid_fetch(&format!("{}:*", max + 1), &items, None).await?;
            let ids = store_rows(store, ctx, folder.id, rows, Some(max)).await?;
            out.changed |= !ids.is_empty();
            out.new_ids.extend(ids);
        }
    } else if sel.exists > 0 {
        let lo = sel.exists.saturating_sub(NEW_CHUNK - 1).max(1);
        let r = conn.raw(&format!("FETCH {lo}:{} {items}", sel.exists)).await?;
        let ids = store_rows(store, ctx, folder.id, super::proto::parse_fetch_rows(&r), None).await?;
        out.changed |= !ids.is_empty();
        // First-sync messages are not "new mail" for notifications.
        out.more_backfill = lo > 1;
    }

    // 3. Backfill older headers (bounded per pass so one huge folder does not starve the others).
    let st2 = load_state(store, folder.id).await?;
    if let Some(min) = st2.min_uid {
        if min > 1 && st2.local_count < sel.exists as i64 {
            let resps = conn.raw(&format!("UID SEARCH UID 1:{}", min - 1)).await?;
            let mut older = super::proto::parse_search(&resps);
            older.sort_unstable();
            let take: Vec<u32> = older.iter().rev().take(BACKFILL_PER_PASS as usize).copied().collect();
            if !take.is_empty() {
                for chunk in take.chunks(NEW_CHUNK as usize) {
                    let (rows, _) = conn.uid_fetch(&uid_set(chunk), &items, None).await?;
                    store_rows(store, ctx, folder.id, rows, None).await?;
                }
                out.changed = true;
                out.more_backfill = older.len() > take.len();
            }
        }
    }

    // 4. Remember where we are.
    {
        let fid = folder.id;
        let uv = sel.uidvalidity;
        let un = sel.uidnext;
        let ms = sel.highest_modseq;
        store
            .call(move |c| {
                c.execute(
                    "UPDATE folders SET uidvalidity=COALESCE(?2, uidvalidity), uidnext=COALESCE(?3, uidnext),
                        highest_modseq=CASE WHEN ?4 IS NULL THEN highest_modseq ELSE ?4 END, last_sync_at=?5 WHERE id=?1",
                    params![fid, uv.map(|v| v as i64), un.map(|v| v as i64), ms.map(|v| v as i64), now_ms()],
                )?;
                Ok(())
            })
            .await?;
    }

    // 5. Prefetch bodies for recent mail (never for Gmail's "All Mail", which duplicates every other folder).
    if ctx.body_days != 0 && folder.role != Some(FolderRole::All) {
        prefetch_bodies(conn, store, folder.id, ctx.body_days).await?;
    }
    Ok(out)
}

pub async fn prefetch_bodies(conn: &mut Conn, store: &Store, folder_id: i64, body_days: i64) -> Result<()> {
    let since = if body_days < 0 { 0 } else { now_ms() - body_days * 86_400_000 };
    let want: Vec<(i64, u32)> = store
        .call(move |c| {
            let mut s = c.prepare(
                "SELECT id, uid FROM messages WHERE folder_id=?1 AND body_cached=0 AND date>=?2 AND size < 26214400 ORDER BY date DESC LIMIT ?3",
            )?;
            let v = s
                .query_map(params![folder_id, since, BODY_PER_PASS], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u32)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(v)
        })
        .await?;
    for chunk in want.chunks(BODY_BATCH) {
        let uids: Vec<u32> = chunk.iter().map(|(_, u)| *u).collect();
        let (rows, _) = conn.uid_fetch(&uid_set(&uids), "(UID BODY.PEEK[])", None).await?;
        let map: Vec<(i64, Vec<u8>)> = rows
            .into_iter()
            .filter_map(|r| {
                let uid = r.uid?;
                let id = chunk.iter().find(|(_, u)| *u == uid)?.0;
                Some((id, r.body?))
            })
            .collect();
        store
            .call(move |c| {
                let tx = c.transaction()?;
                for (id, raw) in map {
                    msgstore::store_body(&tx, id, &raw)?;
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
    }
    Ok(())
}

/// Fetch one body on demand (reading pane). Returns the raw RFC 822 bytes.
pub async fn fetch_body(conn: &mut Conn, store: &Store, id: i64, path: &str, uid: u32) -> Result<Vec<u8>> {
    conn.ensure_selected(path).await?;
    let (rows, _) = conn.uid_fetch(&uid.to_string(), "(UID BODY.PEEK[])", None).await?;
    let raw = rows
        .into_iter()
        .find(|r| r.uid == Some(uid))
        .and_then(|r| r.body)
        .ok_or_else(|| crate::error::ApiError::not_found("The message on the server (it may have been moved or deleted elsewhere)"))?;
    // Saved in the background: the message shows now, even while a sync holds the writer.
    let (store, raw2) = (store.clone(), raw.clone());
    tokio::spawn(async move {
        if let Err(e) = store.call(move |c| msgstore::store_body(c, id, &raw2)).await {
            tracing::warn!(id, "could not cache a fetched body: {}", e.message);
        }
    });
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn internal_date() {
        assert_eq!(parse_internal_date("\"17-Jul-1996 02:44:25 -0700\""), Some(837596665000));
        assert_eq!(parse_internal_date(" 7-Jul-1996 02:44:25 +0000"), Some(836707465000));
    }
    #[test]
    fn roles() {
        assert_eq!(role_from_name("INBOX", "INBOX"), Some(FolderRole::Inbox));
        assert_eq!(role_from_name("INBOX.Sent Items", "Sent Items"), Some(FolderRole::Sent));
        assert_eq!(role_from_name("[Gmail]/All Mail", "All Mail"), Some(FolderRole::All));
        assert_eq!(role_from_name("Projects", "Projects"), None);
    }
}
