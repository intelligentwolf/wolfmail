//! Gmail API adapter (REST v1).
//!
//! Sources (Gmail API reference, fetched 2026-10-28):
//! - users.labels list/create/patch/delete: GET|POST /users/me/labels, PATCH|DELETE /users/me/labels/{id}.
//! - Labels guide: reserved system label ids INBOX, SPAM, TRASH, UNREAD, STARRED, IMPORTANT, SENT, DRAFT,
//!   CATEGORY_PERSONAL/SOCIAL/PROMOTIONS/UPDATES/FORUMS (list not exhaustive).
//! - users.messages.list: GET /users/me/messages, maxResults ≤ 500, pageToken, includeSpamTrash; items carry only
//!   id + threadId; order not stated, so nothing here relies on it.
//! - users.messages.get: GET /users/me/messages/{id}?format=minimal|metadata|raw. minimal = id + labels;
//!   metadata = id, labels and headers; raw = whole message base64url in `raw`. internalDate = epoch ms (string).
//! - users.messages.modify: POST /users/me/messages/{id}/modify {addLabelIds, removeLabelIds} → Message.
//! - users.messages.batchModify: POST /users/me/messages/batchModify, ≤ 1000 ids, empty response.
//! - users.messages.trash / untrash: POST /users/me/messages/{id}/trash|untrash (gmail.modify suffices).
//! - users.messages.delete (permanent) requires https://mail.google.com/ only — NOT granted to WolfMail, so
//!   deleting from Trash is left to Gmail (it empties Trash itself) and the user is told so.
//! - users.messages.send: POST /users/me/messages/send {raw, threadId}; recipients come from To/Cc/Bcc headers.
//! - users.messages.insert: POST /users/me/messages {raw, labelIds} (gmail.modify suffices).
//! - Threads guide: a reply joins a thread when threadId is given, References/In-Reply-To follow RFC 2822 and the
//!   Subject matches.
//! - users.drafts create POST /users/me/drafts {message:{raw,threadId}}, update PUT /users/me/drafts/{id},
//!   delete DELETE /users/me/drafts/{id}, list GET /users/me/drafts (drafts[].message has id + threadId).
//! - users.history.list: GET /users/me/history?startHistoryId=…; messagesAdded/Deleted, labelsAdded/Removed;
//!   an expired start id answers 404 → full sync. users.getProfile: GET /users/me/profile → historyId.
//!
//! Push: Gmail's push notifications go through Cloud Pub/Sub, which needs a server to receive them; WolfMail is a
//! desktop app with no server, so it polls history.list every 30 s while running (a history call with no changes
//! is one small request) and immediately when the user asks for a sync.
use super::store::{self, ApiFolder, ApiMessage};
use super::{bounded, ApiClient, Fetch};
use crate::db::{now_ms, Store};
use crate::error::{ApiError, Result};
use crate::mime;
use crate::model::*;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};

/// Pages of messages.list (500 each) per pass while the first full listing runs.
const BACKFILL_PAGES: usize = 4;
/// Concurrent messages.get calls. Gmail allows 250 quota units/user/s and messages.get costs 5 — 8 in flight stays
/// far below that (Source: Gmail API "Usage limits"; quota units per method).
const PARALLEL_GETS: usize = 4;
const BODY_PREFETCH_PER_PASS: i64 = 50;
/// The synthetic folder every Gmail message row lives in (Gmail has no "All Mail" label id).
pub const ALL_REMOTE: &str = "wolfmail:all";
const ALL_PATH: &str = "\u{1}All Mail";

#[derive(Debug, Deserialize, Clone)]
pub struct Label {
    pub id: String,
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct LabelList {
    #[serde(default)]
    labels: Vec<Label>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MsgRef {
    pub id: String,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub label_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MsgList {
    #[serde(default)]
    messages: Vec<MsgRef>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    #[serde(default)]
    pub headers: Vec<Header>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GMessage {
    pub id: String,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub label_ids: Vec<String>,
    #[serde(default)]
    pub snippet: Option<String>,
    #[serde(default)]
    pub internal_date: Option<String>,
    #[serde(default)]
    pub size_estimate: Option<i64>,
    #[serde(default)]
    pub payload: Option<Payload>,
    #[serde(default)]
    pub raw: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LabelChange {
    message: MsgRef,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryItem {
    #[serde(default)]
    messages_added: Vec<LabelChange>,
    #[serde(default)]
    messages_deleted: Vec<LabelChange>,
    #[serde(default)]
    labels_added: Vec<LabelChange>,
    #[serde(default)]
    labels_removed: Vec<LabelChange>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct HistoryList {
    #[serde(default)]
    history: Vec<HistoryItem>,
    #[serde(default)]
    next_page_token: Option<String>,
    #[serde(default)]
    history_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    history_id: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Draft {
    pub id: String,
    pub message: MsgRef,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DraftList {
    #[serde(default)]
    drafts: Vec<Draft>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// Persisted in accounts.api_state.
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GmailState {
    pub history_id: Option<String>,
    pub backfill_token: Option<String>,
    pub backfill_done: bool,
    pub gen: i64,
}

/// System labels that become folders, with their role and display name. IMPORTANT, CHAT, UNREAD and CATEGORY_*
/// are not folders (UNREAD/STARRED are flags; STARRED also gets a folder like Gmail's own "Starred" view).
pub fn system_label(id: &str) -> Option<(FolderRole, &'static str)> {
    Some(match id {
        "INBOX" => (FolderRole::Inbox, "Inbox"),
        "SENT" => (FolderRole::Sent, "Sent"),
        "DRAFT" => (FolderRole::Drafts, "Drafts"),
        "TRASH" => (FolderRole::Trash, "Trash"),
        "SPAM" => (FolderRole::Junk, "Spam"),
        "STARRED" => (FolderRole::Flagged, "Starred"),
        _ => return None,
    })
}

fn is_user(l: &Label) -> bool {
    l.kind.as_deref() == Some("user")
}

pub fn decode_raw(s: &str) -> Result<Vec<u8>> {
    let t = s.trim_end_matches('=');
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(t)
        .map_err(|e| ApiError::server(format!("Gmail sent an unreadable message body: {e}")))
}

pub fn encode_raw(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Gmail thread id behind a WolfMail thread id ("gt:<threadId>").
pub fn thread_of(wolfmail_thread: &str) -> Option<&str> {
    wolfmail_thread.strip_prefix("gt:")
}

/// Label folders: system labels by id, user labels by name with "/"-nesting (the delimiter Gmail's own IMAP
/// service reports for labels). Returns remote-id → local folder id, plus the All Mail folder id.
pub fn store_labels(c: &rusqlite::Connection, account_id: &str, labels: &[Label], listed_at: i64) -> Result<(HashMap<String, i64>, i64)> {
    let mut map = HashMap::new();
    let mut keep = vec![ALL_REMOTE.to_string()];
    let all = store::upsert_folder(
        c,
        account_id,
        &ApiFolder { remote_id: ALL_REMOTE, path: ALL_PATH, name: "All Mail", role: Some(FolderRole::All), parent_path: None, depth: 0, selectable: true },
    )?;
    for l in labels.iter().filter(|l| !is_user(l)) {
        if let Some((role, name)) = system_label(&l.id) {
            let id = store::upsert_folder(
                c,
                account_id,
                &ApiFolder { remote_id: &l.id, path: &l.id, name, role: Some(role), parent_path: None, depth: 0, selectable: true },
            )?;
            map.insert(l.id.clone(), id);
            keep.push(l.id.clone());
        }
    }
    let mut users: Vec<&Label> = labels.iter().filter(|l| is_user(l)).collect();
    users.sort_by_key(|l| (l.name.matches('/').count(), l.name.clone()));
    let names: BTreeSet<&str> = users.iter().map(|l| l.name.as_str()).collect();
    for l in users {
        let parent = l.name.rsplit_once('/').map(|(p, _)| p).filter(|p| names.contains(p));
        let display = match parent {
            Some(p) => &l.name[p.len() + 1..],
            None => l.name.as_str(),
        };
        let mut depth = 0;
        let mut cur = parent;
        while let Some(p) = cur {
            depth += 1;
            cur = p.rsplit_once('/').map(|(pp, _)| pp).filter(|pp| names.contains(pp));
        }
        let id = store::upsert_folder(
            c,
            account_id,
            &ApiFolder { remote_id: &l.id, path: &l.name, name: display, role: None, parent_path: parent, depth, selectable: true },
        )?;
        map.insert(l.id.clone(), id);
        keep.push(l.id.clone());
    }
    store::remove_folders_except(c, account_id, &keep, listed_at)?;
    Ok((map, all))
}

/// Build what the store needs from a messages.get (metadata or minimal) answer.
pub fn to_api_message(
    c: &rusqlite::Connection,
    m: &GMessage,
    labels: &HashMap<String, i64>,
    label_names: &HashMap<String, String>,
    all_folder: i64,
) -> Result<ApiMessage> {
    let headers: Vec<(String, String)> =
        m.payload.as_ref().map(|p| p.headers.iter().map(|h| (h.name.clone(), h.value.clone())).collect()).unwrap_or_default();
    let header = mime::parse_headers(&store::header_block(&headers));
    let has = |l: &str| m.label_ids.iter().any(|x| x == l);
    let user_names: Vec<String> = m.label_ids.iter().filter_map(|id| label_names.get(id).cloned()).collect();
    let keywords = store::tag_keys_for_names(c, &user_names, false)?;
    let has_attachments = header.multipart_mixed
        || m.payload.as_ref().and_then(|p| p.mime_type.as_deref()).is_some_and(|t| t.eq_ignore_ascii_case("multipart/mixed"));
    Ok(ApiMessage {
        remote_id: m.id.clone(),
        folder_id: all_folder,
        header,
        flags: Flags { seen: !has("UNREAD"), flagged: has("STARRED"), draft: has("DRAFT"), answered: false, forwarded: false },
        keywords,
        size: m.size_estimate.unwrap_or(0),
        internal_date: m.internal_date.as_deref().and_then(|s| s.parse::<i64>().ok()),
        has_attachments,
        ext_thread: m.thread_id.as_ref().map(|t| format!("gt:{t}")),
        snippet: m.snippet.clone().unwrap_or_default(),
        labels: m.label_ids.iter().filter_map(|l| labels.get(l).copied()).collect(),
    })
}

/// Apply a label set (from modify / minimal get / history) to an existing row: flags, tags and label folders.
pub fn apply_label_ids(
    c: &rusqlite::Connection,
    rowid: i64,
    label_ids: &[String],
    labels: &HashMap<String, i64>,
    label_names: &HashMap<String, String>,
) -> Result<()> {
    let has = |l: &str| label_ids.iter().any(|x| x == l);
    let user_names: Vec<String> = label_ids.iter().filter_map(|id| label_names.get(id).cloned()).collect();
    let keywords = store::tag_keys_for_names(c, &user_names, false)?;
    c.execute(
        "UPDATE messages SET seen=?2, flagged=?3, draft=?4, keywords=?5 WHERE id=?1",
        rusqlite::params![rowid, !has("UNREAD"), has("STARRED"), has("DRAFT"), serde_json::to_string(&keywords)?],
    )?;
    let folders: Vec<i64> = label_ids.iter().filter_map(|l| labels.get(l).copied()).collect();
    store::set_labels(c, rowid, &folders)
}

/// The account's label folders as loaded from the local store (for ops between syncs).
pub fn local_label_maps(c: &rusqlite::Connection, account_id: &str) -> Result<LabelMaps> {
    let mut map = HashMap::new();
    let mut names = HashMap::new();
    let mut all = 0;
    let mut st = c.prepare("SELECT id, remote_id, path, role FROM folders WHERE account_id=?1 AND remote_id IS NOT NULL")?;
    let rows = st
        .query_map(rusqlite::params![account_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, rid, path, role) in rows {
        if rid == ALL_REMOTE {
            all = id;
            continue;
        }
        if role.is_none() {
            names.insert(rid.clone(), path);
        }
        map.insert(rid, id);
    }
    Ok((map, names, all))
}

/// (label id → folder id, user label id → name, All Mail folder id).
pub type LabelMaps = (HashMap<String, i64>, HashMap<String, String>, i64);

pub struct PassOutcome {
    pub more: bool,
    pub changed: bool,
    pub new_inbox: Vec<i64>,
}

pub struct Gmail<'a> {
    pub api: &'a ApiClient,
    pub store: &'a Store,
    pub account_id: &'a str,
}

impl Gmail<'_> {
    async fn labels(&self) -> Result<Vec<Label>> {
        Ok(self.api.get_json::<LabelList>("/users/me/labels", &[]).await?.labels)
    }

    async fn get(&self, id: &str, format: &str) -> Result<Option<GMessage>> {
        match self.api.get_json_status::<GMessage>(&format!("/users/me/messages/{id}"), &[("format", format.to_string())], &[], &[404]).await? {
            Fetch::Ok(m) => Ok(Some(m)),
            // Deleted between listing and fetching.
            Fetch::Status(_) => Ok(None),
        }
    }

    async fn label_maps(&self) -> Result<LabelMaps> {
        let listed_at = now_ms();
        let labels = self.labels().await?;
        let aid = self.account_id.to_string();
        let names: HashMap<String, String> = labels.iter().filter(|l| is_user(l)).map(|l| (l.id.clone(), l.name.clone())).collect();
        let (map, all) = self.store.call(move |c| store_labels(c, &aid, &labels, listed_at)).await?;
        Ok((map, names, all))
    }

    /// Fetch and store messages by id. Existing rows only need their labels (minimal); new ones need headers.
    async fn fetch_and_store(
        &self,
        ids: Vec<String>,
        gen: i64,
        maps: &LabelMaps,
    ) -> Result<(Vec<i64>, bool)> {
        if ids.is_empty() {
            return Ok((Vec::new(), false));
        }
        let aid = self.account_id.to_string();
        let ids2 = ids.clone();
        let known: Vec<bool> = self
            .store
            .call(move |c| ids2.iter().map(|i| Ok(store::row_by_remote(c, &aid, i)?.is_some())).collect::<Result<Vec<_>>>())
            .await?;
        let jobs: Vec<(String, bool)> = ids.into_iter().zip(known).collect();
        let fetched = bounded(jobs, PARALLEL_GETS, |(id, known)| async move {
            let r = self.get(&id, if known { "minimal" } else { "metadata" }).await;
            (id, known, r)
        })
        .await;
        let mut got = Vec::new();
        for (id, known, r) in fetched {
            match r? {
                Some(m) => got.push((known, m)),
                None => {
                    let aid = self.account_id.to_string();
                    self.store.call(move |c| store::delete_remote(c, &aid, &id, None)).await?;
                }
            }
        }
        let aid = self.account_id.to_string();
        let (map, names, all) = (maps.0.clone(), maps.1.clone(), maps.2);
        let inbox = map.get("INBOX").copied();
        self.store
            .call(move |c| {
                let tx = c.unchecked_transaction()?;
                let mut new_inbox = Vec::new();
                let mut changed = false;
                for (known, m) in got {
                    if known {
                        if let Some((row, _)) = store::row_by_remote(&tx, &aid, &m.id)? {
                            apply_label_ids(&tx, row, &m.label_ids, &map, &names)?;
                            tx.execute("UPDATE messages SET sync_gen=MAX(sync_gen, ?2) WHERE id=?1", rusqlite::params![row, gen])?;
                            changed = true;
                            continue;
                        }
                    }
                    let am = to_api_message(&tx, &m, &map, &names, all)?;
                    let unread_inbox = !am.flags.seen && inbox.is_some_and(|i| am.labels.contains(&i));
                    let (row, inserted) = store::upsert_message(&tx, &aid, am, gen)?;
                    changed = true;
                    if inserted && unread_inbox {
                        new_inbox.push(row);
                    }
                }
                tx.commit()?;
                Ok((new_inbox, changed))
            })
            .await
    }

    /// One sync pass: labels, history since the stored id, a slice of the full listing, body prefetch.
    pub async fn sync_pass(&self, body_days: i64) -> Result<PassOutcome> {
        let maps = self.label_maps().await?;
        let aid = self.account_id.to_string();
        let mut st: GmailState = self.store.call(move |c| store::account_state(c, &aid)).await?;
        let mut new_inbox = Vec::new();
        let mut changed = false;

        if st.history_id.is_none() {
            // Start: everything after this history id arrives through history.list; everything before through the
            // full listing below. Both are idempotent, so overlap is harmless.
            let p: Profile = self.api.get_json("/users/me/profile", &[]).await?;
            st = GmailState { history_id: Some(p.history_id), backfill_token: None, backfill_done: false, gen: st.gen + 1 };
            self.save(&st).await?;
        }

        // Incremental changes.
        if let Some(start) = st.history_id.clone() {
            let mut page: Option<String> = None;
            let mut adds: BTreeSet<String> = BTreeSet::new();
            let mut dels: BTreeSet<String> = BTreeSet::new();
            let mut latest = None;
            let mut expired = false;
            loop {
                let mut q = vec![("startHistoryId", start.clone()), ("maxResults", "500".to_string())];
                if let Some(t) = &page {
                    q.push(("pageToken", t.clone()));
                }
                match self.api.get_json_status::<HistoryList>("/users/me/history", &q, &[], &[404]).await? {
                    Fetch::Status(_) => {
                        expired = true;
                        break;
                    }
                    Fetch::Ok(h) => {
                        for item in h.history {
                            for a in item.messages_added.into_iter().chain(item.labels_added).chain(item.labels_removed) {
                                adds.insert(a.message.id);
                            }
                            for d in item.messages_deleted {
                                dels.insert(d.message.id);
                            }
                        }
                        if h.history_id.is_some() {
                            latest = h.history_id;
                        }
                        match h.next_page_token {
                            Some(t) => page = Some(t),
                            None => break,
                        }
                    }
                }
            }
            if expired {
                // Source: history.list — an expired startHistoryId answers 404; perform a full sync. The new
                // generation lets the finished listing remove anything deleted meanwhile.
                let p: Profile = self.api.get_json("/users/me/profile", &[]).await?;
                st = GmailState { history_id: Some(p.history_id), backfill_token: None, backfill_done: false, gen: st.gen + 1 };
                self.save(&st).await?;
            } else {
                for d in &dels {
                    adds.remove(d);
                }
                if !dels.is_empty() {
                    let aid = self.account_id.to_string();
                    let dels: Vec<String> = dels.into_iter().collect();
                    self.store
                        .call(move |c| {
                            for d in dels {
                                store::delete_remote(c, &aid, &d, None)?;
                            }
                            Ok(())
                        })
                        .await?;
                    changed = true;
                }
                let (ni, ch) = self.fetch_and_store(adds.into_iter().collect(), st.gen, &maps).await?;
                new_inbox.extend(ni);
                changed |= ch;
                if let Some(l) = latest {
                    st.history_id = Some(l);
                    self.save(&st).await?;
                }
            }
        }

        // Full listing, a few pages per pass.
        if !st.backfill_done {
            for _ in 0..BACKFILL_PAGES {
                let mut q = vec![("maxResults", "500".to_string()), ("includeSpamTrash", "true".to_string())];
                if let Some(t) = &st.backfill_token {
                    q.push(("pageToken", t.clone()));
                }
                let page: MsgList = self.api.get_json("/users/me/messages", &q).await?;
                let ids: Vec<String> = page.messages.into_iter().map(|m| m.id).collect();
                let (_, ch) = self.fetch_and_store(ids, st.gen, &maps).await?;
                changed |= ch;
                st.backfill_token = page.next_page_token;
                if st.backfill_token.is_none() {
                    st.backfill_done = true;
                    let (aid, gen) = (self.account_id.to_string(), st.gen);
                    let swept = self.store.call(move |c| store::sweep_unseen(c, &aid, None, gen)).await?;
                    changed |= swept > 0;
                }
                self.save(&st).await?;
                if st.backfill_done {
                    break;
                }
            }
        }

        // Offline bodies for recent mail.
        let since = if body_days < 0 { 0 } else { now_ms() - body_days * 86_400_000 };
        if body_days != 0 {
            let aid = self.account_id.to_string();
            let want = self.store.call(move |c| store::bodies_wanted(c, &aid, since, BODY_PREFETCH_PER_PASS)).await?;
            let got = bounded(want, PARALLEL_GETS, |(row, rid)| async move { (row, self.raw(&rid).await) }).await;
            for (row, r) in got {
                if let Ok(raw) = r {
                    self.store.call(move |c| store::store_body_and_headers(c, row, &raw)).await?;
                }
            }
        }
        Ok(PassOutcome { more: !st.backfill_done, changed, new_inbox })
    }

    async fn save(&self, st: &GmailState) -> Result<()> {
        let (aid, st) = (self.account_id.to_string(), st.clone());
        self.store.call(move |c| store::set_account_state(c, &aid, &st)).await
    }

    /// Whole message, base64url-decoded.
    pub async fn raw(&self, remote_id: &str) -> Result<Vec<u8>> {
        let m = self
            .get(remote_id, "raw")
            .await?
            .ok_or_else(|| ApiError::not_found("The message on the server (it may have been deleted elsewhere)"))?;
        decode_raw(m.raw.as_deref().unwrap_or_default())
    }

    /// modify one message and mirror the server's resulting labels locally.
    pub async fn modify(&self, rowid: i64, remote_id: &str, add: &[String], remove: &[String]) -> Result<()> {
        let v = self.api.post_json(&format!("/users/me/messages/{remote_id}/modify"), &json!({ "addLabelIds": add, "removeLabelIds": remove })).await?;
        self.mirror(rowid, &v).await
    }

    async fn mirror(&self, rowid: i64, v: &Value) -> Result<()> {
        let ids: Vec<String> = v.get("labelIds").and_then(|l| serde_json::from_value(l.clone()).ok()).unwrap_or_default();
        self.apply_ids(rowid, ids).await
    }

    /// The same label change on many messages: one users.messages.batchModify per 1000 ids (Gmail API reference —
    /// 50 quota units per call instead of 5 per message, so a bulk delete or archive no longer trips the per-user
    /// rate limit; Paul hit "the provider is busy" deleting many messages, 28 Oct 2026). batchModify returns no
    /// body, so each row's resulting labels are computed from its current ones and applied like `mirror` does.
    /// `rows`: (rowid, remote id, current label ids including UNREAD/STARRED).
    pub async fn relabel_many(&self, rows: &[(i64, String, Vec<String>)], add: &[String], remove: &[String]) -> Result<()> {
        let ids: Vec<String> = rows.iter().map(|(_, rid, _)| rid.clone()).collect();
        self.batch_modify(&ids, add, remove).await?;
        for (rowid, _, current) in rows {
            let mut next: Vec<String> = current.iter().filter(|l| !remove.contains(l)).cloned().collect();
            for a in add {
                if !next.contains(a) {
                    next.push(a.clone());
                }
            }
            self.apply_ids(*rowid, next).await?;
        }
        Ok(())
    }

    async fn apply_ids(&self, rowid: i64, ids: Vec<String>) -> Result<()> {
        let aid = self.account_id.to_string();
        self.store
            .call(move |c| {
                let (map, names, _) = local_label_maps(c, &aid)?;
                apply_label_ids(c, rowid, &ids, &map, &names)
            })
            .await
    }

    pub async fn batch_modify(&self, remote_ids: &[String], add: &[String], remove: &[String]) -> Result<()> {
        // Source: batchModify — at most 1000 ids per request.
        for chunk in remote_ids.chunks(1000) {
            self.api.post_json("/users/me/messages/batchModify", &json!({ "ids": chunk, "addLabelIds": add, "removeLabelIds": remove })).await?;
        }
        Ok(())
    }

    pub async fn trash(&self, rowid: i64, remote_id: &str) -> Result<()> {
        let v = self.api.post_empty(&format!("/users/me/messages/{remote_id}/trash")).await?;
        self.mirror(rowid, &v).await
    }

    pub async fn untrash(&self, rowid: i64, remote_id: &str) -> Result<()> {
        let v = self.api.post_empty(&format!("/users/me/messages/{remote_id}/untrash")).await?;
        self.mirror(rowid, &v).await
    }

    /// Send a built message. `thread` is the Gmail thread id of the conversation being answered.
    pub async fn send(&self, raw: &[u8], thread: Option<&str>) -> Result<String> {
        let mut body = json!({ "raw": encode_raw(raw) });
        if let Some(t) = thread {
            body["threadId"] = json!(t);
        }
        let v = self.api.post_json("/users/me/messages/send", &body).await?;
        Ok(v.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string())
    }

    /// Create or replace a draft. Returns the draft id.
    pub async fn save_draft(&self, existing: Option<&str>, raw: &[u8], thread: Option<&str>) -> Result<String> {
        let mut message = json!({ "raw": encode_raw(raw) });
        if let Some(t) = thread {
            message["threadId"] = json!(t);
        }
        let v = match existing {
            Some(id) => self.api.put_json(&format!("/users/me/drafts/{id}"), &json!({ "id": id, "message": message })).await?,
            None => self.api.post_json("/users/me/drafts", &json!({ "message": message })).await?,
        };
        v.get("id").and_then(|i| i.as_str()).map(str::to_string).ok_or_else(|| ApiError::server("Gmail did not return the draft id."))
    }

    pub async fn delete_draft(&self, draft_id: &str) -> Result<()> {
        self.api.delete(&format!("/users/me/drafts/{draft_id}")).await
    }

    /// Draft id of a draft message (drafts.list pages, matching message.id).
    pub async fn find_draft(&self, message_remote_id: &str) -> Result<Option<String>> {
        let mut page: Option<String> = None;
        loop {
            let mut q = vec![("maxResults", "500".to_string())];
            if let Some(t) = &page {
                q.push(("pageToken", t.clone()));
            }
            let l: DraftList = self.api.get_json("/users/me/drafts", &q).await?;
            if let Some(d) = l.drafts.iter().find(|d| d.message.id == message_remote_id) {
                return Ok(Some(d.id.clone()));
            }
            match l.next_page_token {
                Some(t) => page = Some(t),
                None => return Ok(None),
            }
        }
    }

    /// Put a message into this mailbox (cross-account copy). Source: users.messages.insert.
    pub async fn insert(&self, raw: &[u8], label_ids: &[String]) -> Result<()> {
        self.api.post_json("/users/me/messages", &json!({ "raw": encode_raw(raw), "labelIds": label_ids })).await?;
        Ok(())
    }

    /// A user label with this exact name (case-insensitive), created when missing. Returns its id.
    pub async fn ensure_label(&self, name: &str) -> Result<String> {
        if let Some(l) = self.labels().await?.into_iter().find(|l| is_user(l) && l.name.eq_ignore_ascii_case(name)) {
            return Ok(l.id);
        }
        self.create_label(name).await
    }

    pub async fn create_label(&self, name: &str) -> Result<String> {
        // Source: Label resource — labelListVisibility labelShow, messageListVisibility show (visible like a folder).
        let v = self
            .api
            .post_json("/users/me/labels", &json!({ "name": name, "labelListVisibility": "labelShow", "messageListVisibility": "show" }))
            .await?;
        v.get("id").and_then(|i| i.as_str()).map(str::to_string).ok_or_else(|| ApiError::server("Gmail did not return the label id."))
    }

    pub async fn rename_label(&self, id: &str, name: &str) -> Result<()> {
        self.api.patch_json(&format!("/users/me/labels/{id}"), &json!({ "name": name })).await?;
        Ok(())
    }

    pub async fn delete_label(&self, id: &str) -> Result<()> {
        self.api.delete(&format!("/users/me/labels/{id}")).await
    }

    /// Refresh labels and return (label-id → folder id, user label names, All Mail id).
    pub async fn refresh_labels(&self) -> Result<LabelMaps> {
        self.label_maps().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Store;

    fn seed(c: &rusqlite::Connection) {
        c.execute_batch("INSERT INTO accounts(id,name,email,color,provider,imap_json,smtp_json) VALUES ('g','G','g@gmail.com','#c41e1e','gmail','{}','{}');")
            .unwrap();
    }

    fn labels() -> Vec<Label> {
        serde_json::from_value(json!([
            { "id": "INBOX", "name": "INBOX", "type": "system" },
            { "id": "SENT", "name": "SENT", "type": "system" },
            { "id": "TRASH", "name": "TRASH", "type": "system" },
            { "id": "SPAM", "name": "SPAM", "type": "system" },
            { "id": "UNREAD", "name": "UNREAD", "type": "system" },
            { "id": "IMPORTANT", "name": "IMPORTANT", "type": "system" },
            { "id": "CATEGORY_SOCIAL", "name": "CATEGORY_SOCIAL", "type": "system" },
            { "id": "Label_1", "name": "Clients", "type": "user" },
            { "id": "Label_2", "name": "Clients/Acme", "type": "user" },
            { "id": "Label_3", "name": "Orphan/Child", "type": "user" }
        ]))
        .unwrap()
    }

    #[test]
    fn labels_become_folders_with_roles_and_nesting() {
        let s = Store::open_in_memory().unwrap();
        s.with(|c| {
            seed(c);
            let (map, all) = store_labels(c, "g", &labels(), i64::MAX)?;
            assert!(map.contains_key("INBOX") && map.contains_key("SPAM") && map.contains_key("Label_2"));
            assert!(!map.contains_key("UNREAD") && !map.contains_key("IMPORTANT") && !map.contains_key("CATEGORY_SOCIAL"));
            let role: String = c.query_row("SELECT role FROM folders WHERE id=?1", [map["SPAM"]], |r| r.get(0))?;
            assert_eq!(role, "junk");
            let (name, depth, parent): (String, i64, Option<i64>) =
                c.query_row("SELECT name, depth, parent_id FROM folders WHERE id=?1", [map["Label_2"]], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            assert_eq!((name.as_str(), depth, parent), ("Acme", 1, Some(map["Label_1"])));
            // A child whose parent label does not exist keeps its full name at the top level.
            let (name, depth): (String, i64) = c.query_row("SELECT name, depth FROM folders WHERE id=?1", [map["Label_3"]], |r| Ok((r.get(0)?, r.get(1)?)))?;
            assert_eq!((name.as_str(), depth), ("Orphan/Child", 0));
            let all_role: String = c.query_row("SELECT role FROM folders WHERE id=?1", [all], |r| r.get(0))?;
            assert_eq!(all_role, "all");
            // A label deleted on the server disappears locally.
            let fewer: Vec<Label> = labels().into_iter().filter(|l| l.id != "Label_1").collect();
            let (map2, _) = store_labels(c, "g", &fewer, i64::MAX)?;
            assert!(!map2.contains_key("Label_1"));
            let n: i64 = c.query_row("SELECT COUNT(*) FROM folders WHERE remote_id='Label_1'", [], |r| r.get(0))?;
            assert_eq!(n, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn one_row_many_labels_and_trash_hides_it() {
        let s = Store::open_in_memory().unwrap();
        s.with(|c| {
            seed(c);
            let (map, all) = store_labels(c, "g", &labels(), i64::MAX)?;
            let names: HashMap<String, String> = [("Label_1".to_string(), "Clients".to_string())].into_iter().collect();
            let m: GMessage = serde_json::from_value(json!({
                "id": "18c1", "threadId": "18c0", "labelIds": ["INBOX", "UNREAD", "Label_1"], "snippet": "Hi there",
                "internalDate": "1700000000000", "sizeEstimate": 2048,
                "payload": { "mimeType": "multipart/mixed", "headers": [
                    { "name": "From", "value": "Ann <ann@example.com>" }, { "name": "To", "value": "g@gmail.com" },
                    { "name": "Subject", "value": "Quote" }, { "name": "Message-ID", "value": "<q1@example.com>" },
                    { "name": "Date", "value": "Tue, 14 Nov 2023 22:13:20 +0000" }
                ]}
            }))
            .unwrap();
            let am = to_api_message(c, &m, &map, &names, all)?;
            assert!(!am.flags.seen && am.has_attachments);
            assert_eq!(am.ext_thread.as_deref(), Some("gt:18c0"));
            let (row, inserted) = store::upsert_message(c, "g", am, 1)?;
            assert!(inserted);
            let q = |fid: i64| ViewQuery { folder_id: Some(fid.to_string()), unified: None, filter: None, search: None, threaded: false, sort: SortOrder::DateDesc, offset: 0, limit: 10 };
            assert_eq!(crate::msgstore::list_view(c, &q(map["INBOX"]))?.total, 1);
            assert_eq!(crate::msgstore::list_view(c, &q(map["Label_1"]))?.total, 1);
            assert_eq!(crate::msgstore::list_view(c, &q(all))?.total, 1);
            let n: i64 = c.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
            assert_eq!(n, 1, "one row, however many labels");
            let unified = ViewQuery { folder_id: None, unified: Some(UnifiedView::Unread), filter: None, search: None, threaded: false, sort: SortOrder::DateDesc, offset: 0, limit: 10 };
            assert_eq!(crate::msgstore::list_view(c, &unified)?.total, 1);
            // Trashed: Gmail keeps the user label but the message disappears from it, from All Mail and Unread.
            apply_label_ids(c, row, &["TRASH".into(), "UNREAD".into(), "Label_1".into()], &map, &names)?;
            assert_eq!(crate::msgstore::list_view(c, &q(map["Label_1"]))?.total, 0);
            assert_eq!(crate::msgstore::list_view(c, &q(all))?.total, 0);
            assert_eq!(crate::msgstore::list_view(c, &q(map["TRASH"]))?.total, 1);
            assert_eq!(crate::msgstore::list_view(c, &unified)?.total, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn raw_round_trip() {
        let enc = encode_raw(b"Subject: x\r\n\r\nhello \xff");
        assert!(!enc.contains('+') && !enc.contains('/') && !enc.contains('='));
        assert_eq!(decode_raw(&enc).unwrap(), b"Subject: x\r\n\r\nhello \xff");
        assert_eq!(decode_raw("aGk=").unwrap(), b"hi");
        assert_eq!(thread_of("gt:abc"), Some("abc"));
        assert_eq!(thread_of("t:abc"), None);
    }
}
