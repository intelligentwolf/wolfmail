//! Microsoft Graph mail adapter (v1.0).
//!
//! Sources (Microsoft Learn, fetched 2026-10-28):
//! - mailFolder resource: properties id, displayName, parentFolderId, childFolderCount, isHidden; well-known names
//!   inbox, sentitems, drafts, deleteditems, junkemail, archive, outbox … usable in place of an id
//!   (`GET /me/mailFolders/drafts`). List: GET /me/mailFolders (top level only, default page 10 + @odata.nextLink);
//!   children: GET /me/mailFolders/{id}/childFolders. Create: POST /me/mailFolders or …/{id}/childFolders
//!   {displayName}; rename: PATCH /me/mailFolders/{id} {displayName}; delete: DELETE /me/mailFolders/{id}.
//!   Mail search folders (@odata.type #microsoft.graph.mailSearchFolder) are virtual and are not synced.
//! - message delta: GET /me/mailFolders/{id}/messages/delta, $select honoured, Prefer: odata.maxpagesize;
//!   follow @odata.nextLink within a round, keep @odata.deltaLink for the next round; deleted or moved-out items
//!   come back as {"id":…, "@removed":{"reason":"deleted"}}.
//! - Immutable ids: header `Prefer: IdType="ImmutableId"` on every request (ApiClient adds it); the id does not
//!   change when the item moves to another folder of the same mailbox.
//! - GET /me/messages/{id}/$value → MIME. PATCH /me/messages/{id} {isRead | flag{flagStatus} | categories}.
//! - POST /me/messages/{id}/move|copy {destinationId} (folder id or well-known name) → 201 message.
//! - DELETE /me/messages/{id} → 204; POST /me/messages/{id}/permanentDelete → 204 (to the purges folder).
//! - POST /me/sendMail with Content-Type text/plain and the base64 MIME (To/Cc/Bcc/Subject from its headers);
//!   saves to Sent Items; 202. POST /me/messages with base64 MIME creates a draft in Drafts (201, isDraft true).
//! - Throttling: 429 + Retry-After (handled in ApiClient).
//!
//! Threading: Graph's conversationId groups a conversation; it is used as the authoritative thread id ("gc:…").
//! Replies are sent as MIME built by WolfMail with In-Reply-To/References, so recipients' clients thread them too.
use super::store::{self, ApiFolder, ApiMessage};
use super::{ApiClient, Fetch};
use crate::db::{now_ms, Store};
use crate::error::{ApiError, Result};
use crate::mime::HeaderInfo;
use crate::model::*;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;

const PAGE_SIZE: &str = "odata.maxpagesize=100";
const INBOX_PAGES: usize = 10;
const OTHER_PAGES: usize = 3;
const BODY_PREFETCH_PER_PASS: i64 = 50;
/// Properties requested from delta (Source: message resource property names).
const SELECT: &str = "subject,from,toRecipients,ccRecipients,bccRecipients,replyTo,receivedDateTime,sentDateTime,internetMessageId,conversationId,isRead,isDraft,flag,categories,hasAttachments,bodyPreview";

/// Well-known folder names → WolfMail roles.
pub const WELL_KNOWN: &[(&str, FolderRole)] = &[
    ("inbox", FolderRole::Inbox),
    ("sentitems", FolderRole::Sent),
    ("drafts", FolderRole::Drafts),
    ("deleteditems", FolderRole::Trash),
    ("junkemail", FolderRole::Junk),
    ("archive", FolderRole::Archive),
    ("outbox", FolderRole::Outbox),
];

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MailFolder {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub child_folder_count: i64,
    #[serde(rename = "@odata.type", default)]
    pub odata_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FolderPage {
    #[serde(default)]
    value: Vec<MailFolder>,
    #[serde(rename = "@odata.nextLink", default)]
    next: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct EmailAddress {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub address: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Recipient {
    #[serde(default)]
    pub email_address: EmailAddress,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct FollowupFlag {
    #[serde(default)]
    pub flag_status: Option<String>,
}

/// A message as returned by delta / get. Every field optional: delta updates may carry only what changed.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct GraphMessage {
    pub id: String,
    #[serde(rename = "@removed", default)]
    pub removed: Option<Value>,
    pub subject: Option<String>,
    pub from: Option<Recipient>,
    pub to_recipients: Option<Vec<Recipient>>,
    pub cc_recipients: Option<Vec<Recipient>>,
    pub bcc_recipients: Option<Vec<Recipient>>,
    pub reply_to: Option<Vec<Recipient>>,
    pub received_date_time: Option<String>,
    pub sent_date_time: Option<String>,
    pub internet_message_id: Option<String>,
    pub conversation_id: Option<String>,
    pub is_read: Option<bool>,
    pub is_draft: Option<bool>,
    pub flag: Option<FollowupFlag>,
    pub categories: Option<Vec<String>>,
    pub has_attachments: Option<bool>,
    pub body_preview: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeltaPage {
    #[serde(default)]
    value: Vec<GraphMessage>,
    #[serde(rename = "@odata.nextLink", default)]
    next: Option<String>,
    #[serde(rename = "@odata.deltaLink", default)]
    delta: Option<String>,
}

/// Per-folder delta state (folders.sync_state).
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FolderSync {
    /// nextLink (round in progress) or deltaLink (round complete).
    pub link: Option<String>,
    /// True once a round has completed (so rows not seen in the first full round can be swept).
    pub round_done: bool,
    pub gen: i64,
}

/// Account state: well-known folder ids (resolved once; folder ids are constant — Source: immutable-id doc,
/// "Container types … their regular IDs were already constant").
#[derive(Debug, Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GraphState {
    pub roles: HashMap<String, String>,
    pub roles_resolved: bool,
}

fn addr(r: &Recipient) -> Option<Address> {
    let email = r.email_address.address.clone()?.trim().to_string();
    if email.is_empty() {
        return None;
    }
    Some(Address { name: r.email_address.name.clone().filter(|n| !n.trim().is_empty() && !n.eq_ignore_ascii_case(&email)), email })
}

fn addrs(v: &Option<Vec<Recipient>>) -> Vec<Address> {
    v.as_ref().map(|l| l.iter().filter_map(addr).collect()).unwrap_or_default()
}

fn parse_time(s: &Option<String>) -> Option<i64> {
    s.as_deref().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).map(|d| d.timestamp_millis())
}

/// HeaderInfo from Graph JSON (the list API exposes no References / List-Unsubscribe; those are filled in when the
/// MIME body is fetched — store::store_body_and_headers).
pub fn header_from(m: &GraphMessage) -> HeaderInfo {
    HeaderInfo {
        subject: m.subject.clone().unwrap_or_default(),
        from: m.from.as_ref().and_then(addr).unwrap_or_default(),
        to: addrs(&m.to_recipients),
        cc: addrs(&m.cc_recipients),
        bcc: addrs(&m.bcc_recipients),
        reply_to: addrs(&m.reply_to),
        date: parse_time(&m.sent_date_time).or_else(|| parse_time(&m.received_date_time)),
        message_id: m.internet_message_id.as_deref().map(|s| s.trim().trim_start_matches('<').trim_end_matches('>').to_string()).filter(|s| !s.is_empty()),
        in_reply_to: None,
        references: Vec::new(),
        list_unsubscribe: None,
        auth_results: None,
        multipart_mixed: false,
    }
}

pub fn flagged(m: &GraphMessage) -> Option<bool> {
    m.flag.as_ref().map(|f| f.flag_status.as_deref() == Some("flagged"))
}

/// Store one delta item for `folder_id`. Existing rows get only the fields present in the item.
pub fn apply_item(c: &rusqlite::Connection, account_id: &str, folder_id: i64, m: &GraphMessage, gen: i64) -> Result<Option<(i64, bool)>> {
    if m.removed.is_some() {
        store::delete_remote(c, account_id, &m.id, Some(folder_id))?;
        return Ok(None);
    }
    if let Some((row, cur_folder)) = store::row_by_remote(c, account_id, &m.id)? {
        if cur_folder != folder_id {
            let uid: i64 = c.query_row("SELECT COALESCE(MAX(uid),0)+1 FROM messages WHERE folder_id=?1", [folder_id], |r| r.get(0))?;
            c.execute("UPDATE messages SET folder_id=?2, uid=?3 WHERE id=?1", rusqlite::params![row, folder_id, uid])?;
        }
        if let Some(r) = m.is_read {
            c.execute("UPDATE messages SET seen=?2 WHERE id=?1", rusqlite::params![row, r])?;
        }
        if let Some(f) = flagged(m) {
            c.execute("UPDATE messages SET flagged=?2 WHERE id=?1", rusqlite::params![row, f])?;
        }
        if let Some(d) = m.is_draft {
            c.execute("UPDATE messages SET draft=?2 WHERE id=?1", rusqlite::params![row, d])?;
        }
        if let Some(cats) = &m.categories {
            let keys = store::tag_keys_for_names(c, cats, true)?;
            c.execute("UPDATE messages SET keywords=?2 WHERE id=?1", rusqlite::params![row, serde_json::to_string(&keys)?])?;
        }
        c.execute("UPDATE messages SET sync_gen=MAX(sync_gen, ?2) WHERE id=?1", rusqlite::params![row, gen])?;
        return Ok(Some((row, false)));
    }
    let keywords = store::tag_keys_for_names(c, m.categories.as_deref().unwrap_or_default(), true)?;
    let am = ApiMessage {
        remote_id: m.id.clone(),
        folder_id,
        header: header_from(m),
        flags: Flags {
            seen: m.is_read.unwrap_or(false),
            flagged: flagged(m).unwrap_or(false),
            draft: m.is_draft.unwrap_or(false),
            answered: false,
            forwarded: false,
        },
        keywords,
        size: 0,
        internal_date: parse_time(&m.received_date_time),
        has_attachments: m.has_attachments.unwrap_or(false),
        ext_thread: m.conversation_id.as_ref().map(|c| format!("gc:{c}")),
        snippet: m.body_preview.clone().unwrap_or_default(),
        labels: Vec::new(),
    };
    Ok(Some(store::upsert_message(c, account_id, am, gen)?))
}

pub struct PassOutcome {
    pub more: bool,
    pub changed: Vec<i64>,
    pub new_inbox: Vec<i64>,
}

pub struct Graph<'a> {
    pub api: &'a ApiClient,
    pub store: &'a Store,
    pub account_id: &'a str,
}

impl Graph<'_> {
    async fn folder_pages(&self, first: &str) -> Result<Vec<MailFolder>> {
        let mut out = Vec::new();
        let mut url = first.to_string();
        loop {
            let p: FolderPage = self.api.get_json(&url, &[]).await?;
            out.extend(p.value);
            match p.next {
                Some(n) => url = n,
                None => return Ok(out),
            }
        }
    }

    /// The whole folder tree (each parent's childFolders), excluding search folders. Returns (folder, path, depth).
    async fn folder_tree(&self) -> Result<Vec<(MailFolder, String, Option<String>, i64)>> {
        let mut out = Vec::new();
        let mut queue: Vec<(MailFolder, Option<String>, i64)> =
            self.folder_pages("/me/mailFolders?$top=100").await?.into_iter().map(|f| (f, None, 0)).collect();
        while let Some((f, parent_path, depth)) = queue.pop() {
            if f.odata_type.as_deref().is_some_and(|t| t.contains("mailSearchFolder")) {
                continue;
            }
            let path = match &parent_path {
                Some(p) => format!("{p}/{}", f.display_name),
                None => f.display_name.clone(),
            };
            if f.child_folder_count > 0 {
                for child in self.folder_pages(&format!("/me/mailFolders/{}/childFolders?$top=100", f.id)).await? {
                    queue.push((child, Some(path.clone()), depth + 1));
                }
            }
            out.push((f, path, parent_path, depth));
        }
        out.sort_by_key(|(_, _, _, d)| *d);
        Ok(out)
    }

    async fn resolve_roles(&self) -> Result<HashMap<String, String>> {
        let aid = self.account_id.to_string();
        let mut st: GraphState = self.store.call(move |c| store::account_state(c, &aid)).await?;
        if st.roles_resolved {
            return Ok(st.roles);
        }
        let mut roles = HashMap::new();
        for (wk, role) in WELL_KNOWN {
            // A mailbox may lack e.g. the archive folder: 404 means "no such role here".
            if let Fetch::Ok(v) = self.api.get_json_status::<Value>(&format!("/me/mailFolders/{wk}"), &[("$select", "id".to_string())], &[], &[404]).await? {
                if let Some(id) = v.get("id").and_then(|i| i.as_str()) {
                    roles.insert(id.to_string(), role.as_str().to_string());
                }
            }
        }
        st.roles = roles.clone();
        st.roles_resolved = true;
        let aid = self.account_id.to_string();
        self.store.call(move |c| store::set_account_state(c, &aid, &st)).await?;
        Ok(roles)
    }

    /// Sync the folder list. Returns (local id, remote id, role) for every folder, Inbox first.
    pub async fn sync_folders(&self) -> Result<Vec<(i64, String, Option<FolderRole>)>> {
        let roles = self.resolve_roles().await?;
        let tree = self.folder_tree().await?;
        let aid = self.account_id.to_string();
        self.store
            .call(move |c| {
                let tx = c.unchecked_transaction()?;
                let mut keep = Vec::new();
                let mut out = Vec::new();
                for (f, path, parent, depth) in &tree {
                    let role = roles.get(&f.id).and_then(|r| FolderRole::parse(r));
                    let id = store::upsert_folder(
                        &tx,
                        &aid,
                        &ApiFolder { remote_id: &f.id, path, name: &f.display_name, role, parent_path: parent.as_deref(), depth: *depth, selectable: true },
                    )?;
                    keep.push(f.id.clone());
                    out.push((id, f.id.clone(), role));
                }
                store::remove_folders_except(&tx, &aid, &keep)?;
                tx.commit()?;
                out.sort_by_key(|(_, _, r)| *r != Some(FolderRole::Inbox));
                Ok(out)
            })
            .await
    }

    /// One delta round slice for a folder.
    async fn sync_folder(&self, folder_id: i64, remote: &str, pages: usize) -> Result<(bool, bool, Vec<i64>)> {
        let mut st: FolderSync = self.store.call(move |c| store::folder_state(c, folder_id)).await?;
        if st.gen == 0 {
            st.gen = 1;
        }
        let mut changed = false;
        let mut new_ids = Vec::new();
        for _ in 0..pages {
            let url = st.link.clone().unwrap_or_else(|| format!("/me/mailFolders/{remote}/messages/delta?$select={SELECT}"));
            let page = match self.api.get_json_status::<DeltaPage>(&url, &[], &[PAGE_SIZE], &[410]).await? {
                Fetch::Ok(p) => p,
                Fetch::Status(_) => {
                    // 410 Gone: the delta token is no longer valid — start a fresh round; rows not seen in it are
                    // swept when it completes.
                    st = FolderSync { link: None, round_done: false, gen: st.gen + 1 };
                    let s2 = st.clone();
                    self.store.call(move |c| store::set_folder_state(c, folder_id, &s2)).await?;
                    continue;
                }
            };
            let (aid, gen, items) = (self.account_id.to_string(), st.gen, page.value);
            let (ch, ids) = self
                .store
                .call(move |c| {
                    let tx = c.unchecked_transaction()?;
                    let mut ids = Vec::new();
                    let mut ch = false;
                    for m in &items {
                        ch = true;
                        if let Some((row, inserted)) = apply_item(&tx, &aid, folder_id, m, gen)? {
                            if inserted && !m.is_read.unwrap_or(true) {
                                ids.push(row);
                            }
                        }
                    }
                    tx.commit()?;
                    Ok((ch, ids))
                })
                .await?;
            changed |= ch;
            new_ids.extend(ids);
            if let Some(n) = page.next {
                st.link = Some(n);
            } else if let Some(d) = page.delta {
                st.link = Some(d);
                if !st.round_done {
                    st.round_done = true;
                    let (aid, gen) = (self.account_id.to_string(), st.gen);
                    let swept = self.store.call(move |c| store::sweep_unseen(c, &aid, Some(folder_id), gen)).await?;
                    changed |= swept > 0;
                }
                let s2 = st.clone();
                self.store.call(move |c| store::set_folder_state(c, folder_id, &s2)).await?;
                return Ok((false, changed, new_ids));
            }
            let s2 = st.clone();
            self.store.call(move |c| store::set_folder_state(c, folder_id, &s2)).await?;
        }
        // The page budget ran out mid-round (only a deltaLink ends a round, and that returned above): more next pass.
        Ok((true, changed, new_ids))
    }

    pub async fn sync_pass(&self, body_days: i64) -> Result<PassOutcome> {
        let folders = self.sync_folders().await?;
        let mut out = PassOutcome { more: false, changed: Vec::new(), new_inbox: Vec::new() };
        for (fid, remote, role) in folders {
            let pages = if role == Some(FolderRole::Inbox) { INBOX_PAGES } else { OTHER_PAGES };
            let (more, changed, new_ids) = match self.sync_folder(fid, &remote, pages).await {
                Ok(r) => r,
                // A folder deleted meanwhile must not stop the others.
                Err(e) if e.kind == crate::error::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            out.more |= more;
            if changed {
                out.changed.push(fid);
            }
            if role == Some(FolderRole::Inbox) {
                out.new_inbox.extend(new_ids);
            }
        }
        if body_days != 0 {
            let since = if body_days < 0 { 0 } else { now_ms() - body_days * 86_400_000 };
            let aid = self.account_id.to_string();
            let want = self.store.call(move |c| store::bodies_wanted(c, &aid, since, BODY_PREFETCH_PER_PASS)).await?;
            let got = super::bounded(want, 6, |(row, rid)| async move { (row, self.raw(&rid).await) }).await;
            for (row, r) in got {
                if let Ok(raw) = r {
                    self.store.call(move |c| store::store_body_and_headers(c, row, &raw)).await?;
                }
            }
        }
        Ok(out)
    }

    pub async fn raw(&self, remote_id: &str) -> Result<Vec<u8>> {
        self.api.get_bytes(&format!("/me/messages/{remote_id}/$value"), &[]).await
    }

    pub async fn patch(&self, remote_id: &str, body: Value) -> Result<()> {
        self.api.patch_json(&format!("/me/messages/{remote_id}"), &body).await?;
        Ok(())
    }

    /// Move; returns the message's id afterwards (the same with immutable ids).
    pub async fn move_to(&self, remote_id: &str, dest: &str) -> Result<String> {
        let v = self.api.post_json(&format!("/me/messages/{remote_id}/move"), &json!({ "destinationId": dest })).await?;
        Ok(v.get("id").and_then(|i| i.as_str()).unwrap_or(remote_id).to_string())
    }

    pub async fn copy_to(&self, remote_id: &str, dest: &str) -> Result<()> {
        self.api.post_json(&format!("/me/messages/{remote_id}/copy"), &json!({ "destinationId": dest })).await?;
        Ok(())
    }

    pub async fn delete(&self, remote_id: &str) -> Result<()> {
        self.api.delete(&format!("/me/messages/{remote_id}")).await
    }

    pub async fn permanent_delete(&self, remote_id: &str) -> Result<()> {
        self.api.post_empty(&format!("/me/messages/{remote_id}/permanentDelete")).await?;
        Ok(())
    }

    /// MIME as Graph wants it: standard base64 text (Source: sendMail — "MIME content … encoded in base64").
    pub fn mime_body(raw: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(raw)
    }

    pub async fn send(&self, raw: &[u8]) -> Result<()> {
        self.api.post_text("/me/sendMail", Self::mime_body(raw)).await?;
        Ok(())
    }

    /// New draft from MIME in Drafts; returns its id.
    pub async fn create_draft(&self, raw: &[u8]) -> Result<String> {
        let v = self.api.post_text("/me/messages", Self::mime_body(raw)).await?;
        v.get("id").and_then(|i| i.as_str()).map(str::to_string).ok_or_else(|| ApiError::server("Microsoft did not return the draft id."))
    }

    pub async fn create_folder(&self, parent_remote: Option<&str>, name: &str) -> Result<String> {
        let path = match parent_remote {
            Some(p) => format!("/me/mailFolders/{p}/childFolders"),
            None => "/me/mailFolders".to_string(),
        };
        let v = self.api.post_json(&path, &json!({ "displayName": name })).await?;
        v.get("id").and_then(|i| i.as_str()).map(str::to_string).ok_or_else(|| ApiError::server("Microsoft did not return the folder id."))
    }

    pub async fn rename_folder(&self, remote: &str, name: &str) -> Result<()> {
        self.api.patch_json(&format!("/me/mailFolders/{remote}"), &json!({ "displayName": name })).await?;
        Ok(())
    }

    pub async fn delete_folder(&self, remote: &str) -> Result<()> {
        self.api.delete(&format!("/me/mailFolders/{remote}")).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Store;

    #[test]
    fn delta_items_insert_update_move_remove() {
        let s = Store::open_in_memory().unwrap();
        s.with(|c| {
            c.execute_batch(
                "INSERT INTO accounts(id,name,email,color,provider,imap_json,smtp_json) VALUES ('o','O','o@contoso.com','#c41e1e','outlook','{}','{}');
                 INSERT INTO folders(id,account_id,path,name,role,remote_id) VALUES (1,'o','Inbox','Inbox','inbox','F-IN'), (2,'o','Projects','Projects',NULL,'F-PR');",
            )?;
            let m: GraphMessage = serde_json::from_value(json!({
                "id": "AAk1", "subject": "Hello", "from": { "emailAddress": { "name": "Ann", "address": "ann@example.com" } },
                "toRecipients": [{ "emailAddress": { "name": "o@contoso.com", "address": "o@contoso.com" } }],
                "receivedDateTime": "2026-10-28T09:00:00Z", "sentDateTime": "2026-10-28T08:59:00Z",
                "internetMessageId": "<abc@example.com>", "conversationId": "CONV1", "isRead": false, "isDraft": false,
                "flag": { "flagStatus": "flagged" }, "categories": ["Blue category"], "hasAttachments": true, "bodyPreview": "Hi"
            }))
            .unwrap();
            let (row, inserted) = apply_item(c, "o", 1, &m, 1)?.unwrap();
            assert!(inserted);
            let (subj, mid, tid, flagged, kw, to): (String, String, String, bool, String, String) = c.query_row(
                "SELECT subject, message_id, thread_id, flagged, keywords, to_json FROM messages WHERE id=?1",
                [row],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )?;
            assert_eq!((subj.as_str(), mid.as_str(), tid.as_str(), flagged), ("Hello", "abc@example.com", "gc:CONV1", true));
            assert!(kw.contains("cat_blue_category"), "an Outlook category becomes a tag: {kw}");
            assert!(!to.contains("\"name\""), "a display name equal to the address is dropped: {to}");
            // A delta update carrying only isRead must not wipe the categories.
            let upd: GraphMessage = serde_json::from_value(json!({ "id": "AAk1", "isRead": true })).unwrap();
            apply_item(c, "o", 1, &upd, 1)?;
            let (seen, kw2): (bool, String) = c.query_row("SELECT seen, keywords FROM messages WHERE id=?1", [row], |r| Ok((r.get(0)?, r.get(1)?)))?;
            assert!(seen);
            assert_eq!(kw, kw2);
            // Moved to Projects: the destination delta claims the row, then the source's @removed must not delete it.
            apply_item(c, "o", 2, &upd, 1)?;
            let removed: GraphMessage = serde_json::from_value(json!({ "id": "AAk1", "@removed": { "reason": "deleted" } })).unwrap();
            apply_item(c, "o", 1, &removed, 1)?;
            let folder: i64 = c.query_row("SELECT folder_id FROM messages WHERE id=?1", [row], |r| r.get(0))?;
            assert_eq!(folder, 2);
            // Removed in the folder that holds it: gone.
            apply_item(c, "o", 2, &removed, 1)?;
            let n: i64 = c.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
            assert_eq!(n, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn mime_body_is_standard_base64() {
        assert_eq!(Graph::mime_body(b"\xfb\xff"), "+/8=");
    }
}
