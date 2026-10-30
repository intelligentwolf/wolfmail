//! Engine commands for accounts on a provider API (Gmail / Microsoft Graph). messages.rs, compose.rs and
//! accounts.rs split each request by backend and hand the API part here; IMAP accounts never reach this file.
// Source: providers/gmail.rs and providers/graph.rs (endpoint citations live there); docs/API.md commands.
use super::Engine;
use crate::error::{ApiError, Result};
use crate::model::*;
use crate::oauth;
use crate::providers::gmail::{self, Gmail};
use crate::providers::graph::Graph;
use crate::providers::store::{self, RemoteRow};
use crate::providers::{backend, ApiClient, Backend};
use rusqlite::params;
use std::collections::BTreeMap;

/// What undo needs to put API messages back.
pub(crate) enum ApiUndo {
    /// Per message (rowid, remote id, labels added, labels removed): undo removes what was added and re-adds what
    /// was removed. `untrash` instead untrashes (Gmail trash is its own call, not a label edit).
    Gmail { account_id: String, items: Vec<(i64, String, Vec<String>, Vec<String>)>, untrash: bool },
    /// Move each message back to the folder it came from (remote folder id).
    Graph { account_id: String, items: Vec<(i64, String, String)> },
}

/// Rows of one API account.
pub(crate) struct ApiGroup {
    pub account: Account,
    pub backend: Backend,
    pub rows: Vec<i64>,
}

const CAN_NOT_PURGE_GMAIL: &str = "Gmail removes messages from Trash by itself after 30 days. WolfMail's Gmail access \
    (the same Google approval IntelligentWolf uses) can move mail to Trash but not erase it for good — empty Trash \
    in Gmail on the web if you need it gone now.";

pub(crate) const CAN_NOT_EMPTY_GMAIL: &str = "Gmail empties Trash and Spam by itself after 30 days. WolfMail's Gmail access (the same Google \
    approval IntelligentWolf uses) can move mail to Trash but not erase it for good — empty them in Gmail on the web if you need it gone now.";

impl Engine {
    pub(crate) async fn api_client(&self, acct: &Account) -> Result<ApiClient> {
        let token = oauth::access_token(&self.inner.http, self.inner.secrets.as_ref(), &acct.email).await.map_err(|e| e.with_account(&acct.id))?;
        Ok(match backend(acct) {
            Backend::Gmail => ApiClient::new(&self.inner.http, token, &self.inner.api_bases.gmail, false, &acct.email),
            _ => ApiClient::new(&self.inner.http, token, &self.inner.api_bases.graph, true, &acct.email),
        })
    }

    /// Split message ids into IMAP ids (returned as given) and per-account API groups.
    pub(crate) async fn api_partition(&self, ids: &[String]) -> Result<(Vec<String>, Vec<ApiGroup>)> {
        let accounts = self.accounts_list().await?;
        let parsed: Vec<(String, i64)> = ids
            .iter()
            .map(|s| s.parse::<i64>().map(|i| (s.clone(), i)).map_err(|_| ApiError::not_found("A message")))
            .collect::<Result<_>>()?;
        let owners: Vec<(String, i64, String)> = self
            .store()
            .call(move |c| {
                let mut v = Vec::new();
                for (s, id) in parsed {
                    let a: String = c
                        .query_row("SELECT account_id FROM messages WHERE id=?1", params![id], |r| r.get(0))
                        .map_err(|_| ApiError::not_found("A message"))?;
                    v.push((s, id, a));
                }
                Ok(v)
            })
            .await?;
        let mut imap = Vec::new();
        let mut groups: BTreeMap<String, ApiGroup> = BTreeMap::new();
        for (s, id, aid) in owners {
            let acct = accounts.iter().find(|a| a.id == aid).ok_or_else(|| ApiError::not_found("The account"))?;
            match backend(acct) {
                Backend::Imap => imap.push(s),
                b => groups.entry(aid.clone()).or_insert_with(|| ApiGroup { account: acct.clone(), backend: b, rows: Vec::new() }).rows.push(id),
            }
        }
        Ok((imap, groups.into_values().collect()))
    }

    async fn remote_rows(&self, rows: &[i64]) -> Result<Vec<RemoteRow>> {
        let rows = rows.to_vec();
        self.store().read(move |c| rows.iter().map(|r| store::remote_row(c, *r)).collect()).await
    }

    pub(crate) async fn account_folder_ids(&self, account_id: &str) -> Result<Vec<String>> {
        let aid = account_id.to_string();
        self.store()
            .call(move |c| {
                let mut st = c.prepare("SELECT id FROM folders WHERE account_id=?1")?;
                let v = st.query_map(params![aid], |r| Ok(r.get::<_, i64>(0)?.to_string()))?.collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await
    }

    /// Raw message bytes for an API row (cached afterwards).
    pub(crate) async fn api_raw(&self, rowid: i64) -> Result<Vec<u8>> {
        let r = self.remote_rows(&[rowid]).await?.remove(0);
        let acct = self.account(&r.account_id).await?;
        let api = self.api_client(&acct).await?;
        let raw = match backend(&acct) {
            Backend::Gmail => Gmail { api: &api, store: self.store(), account_id: &acct.id }.raw(&r.remote_id).await?,
            _ => Graph { api: &api, store: self.store(), account_id: &acct.id }.raw(&r.remote_id).await?,
        };
        // Saved in the background: the message shows now, even while a sync holds the writer.
        let (store, raw2) = (self.store().clone(), raw.clone());
        tokio::spawn(async move {
            if let Err(e) = store.call(move |c| store::store_body_and_headers(c, rowid, &raw2)).await {
                tracing::warn!(rowid, "could not cache a fetched body: {}", e.message);
            }
        });
        Ok(raw)
    }

    pub(crate) async fn api_set_flags(&self, g: &ApiGroup, add: &[String], remove: &[String]) -> Result<()> {
        let api = self.api_client(&g.account).await?;
        let rows = self.remote_rows(&g.rows).await?;
        match g.backend {
            Backend::Gmail => {
                // UNREAD / STARRED are the Gmail labels behind seen / flagged; answered, forwarded and draft have no
                // Gmail equivalent and stay local (as Gmail's own IMAP service does with \Answered).
                let mut add_l = Vec::new();
                let mut rem_l = Vec::new();
                for f in add {
                    match f.as_str() {
                        "seen" => rem_l.push("UNREAD".to_string()),
                        "flagged" => add_l.push("STARRED".to_string()),
                        _ => {}
                    }
                }
                for f in remove {
                    match f.as_str() {
                        "seen" => add_l.push("UNREAD".to_string()),
                        "flagged" => rem_l.push("STARRED".to_string()),
                        _ => {}
                    }
                }
                if !add_l.is_empty() || !rem_l.is_empty() {
                    let gm = Gmail { api: &api, store: self.store(), account_id: &g.account.id };
                    for r in &rows {
                        gm.modify(r.rowid, &r.remote_id, &add_l, &rem_l).await?;
                    }
                }
            }
            _ => {
                let gr = Graph { api: &api, store: self.store(), account_id: &g.account.id };
                let mut body = serde_json::Map::new();
                for (f, v) in add.iter().map(|f| (f, true)).chain(remove.iter().map(|f| (f, false))) {
                    match f.as_str() {
                        "seen" => {
                            body.insert("isRead".into(), v.into());
                        }
                        "flagged" => {
                            body.insert("flag".into(), serde_json::json!({ "flagStatus": if v { "flagged" } else { "notFlagged" } }));
                        }
                        _ => {}
                    }
                }
                if !body.is_empty() {
                    for r in &rows {
                        gr.patch(&r.remote_id, serde_json::Value::Object(body.clone())).await?;
                    }
                }
            }
        }
        // Local mirror for every flag (answered/forwarded live only here for API accounts).
        let (ids, add, remove) = (g.rows.clone(), add.to_vec(), remove.to_vec());
        self.store()
            .call(move |c| {
                let tx = c.transaction()?;
                for id in ids {
                    for f in &add {
                        tx.execute(&format!("UPDATE messages SET {f}=1 WHERE id=?1"), params![id])?;
                    }
                    for f in &remove {
                        tx.execute(&format!("UPDATE messages SET {f}=0 WHERE id=?1"), params![id])?;
                    }
                }
                tx.commit()?;
                Ok(())
            })
            .await
    }

    pub(crate) async fn api_set_tags(&self, g: &ApiGroup, add: &[String], remove: &[String]) -> Result<()> {
        let api = self.api_client(&g.account).await?;
        let rows = self.remote_rows(&g.rows).await?;
        let keys: Vec<String> = add.iter().chain(remove.iter()).cloned().collect();
        let names: BTreeMap<String, String> = self
            .store()
            .call(move |c| {
                let mut m = BTreeMap::new();
                for k in keys {
                    let n = store::tag_name(c, &k)?.unwrap_or_else(|| k.clone());
                    m.insert(k, n);
                }
                Ok(m)
            })
            .await?;
        match g.backend {
            Backend::Gmail => {
                // Tags are Gmail user labels with the tag's name (created when missing).
                let gm = Gmail { api: &api, store: self.store(), account_id: &g.account.id };
                let mut add_ids = Vec::new();
                for k in add {
                    add_ids.push(gm.ensure_label(&names[k]).await?);
                }
                let mut rem_ids = Vec::new();
                for k in remove {
                    rem_ids.push(gm.ensure_label(&names[k]).await?);
                }
                gm.refresh_labels().await?;
                for r in &rows {
                    gm.modify(r.rowid, &r.remote_id, &add_ids, &rem_ids).await?;
                }
            }
            _ => {
                // Tags are Outlook categories (names).
                let gr = Graph { api: &api, store: self.store(), account_id: &g.account.id };
                for r in &rows {
                    // Categories PATCH replaces the whole list: start from the message's current tags (as names).
                    let kw = r.keywords.clone();
                    let mut cats: Vec<String> = Vec::new();
                    for n in self.store().call(move |c| kw.iter().map(|k| Ok(store::tag_name(c, k)?.unwrap_or_else(|| k.clone()))).collect::<Result<Vec<_>>>()).await? {
                        if !cats.iter().any(|c| c.eq_ignore_ascii_case(&n)) {
                            cats.push(n);
                        }
                    }
                    cats.retain(|c| !remove.iter().any(|k| names[k].eq_ignore_ascii_case(c)));
                    for k in add {
                        if !cats.iter().any(|c| c.eq_ignore_ascii_case(&names[k])) {
                            cats.push(names[k].clone());
                        }
                    }
                    gr.patch(&r.remote_id, serde_json::json!({ "categories": cats })).await?;
                }
            }
        }
        let (ids, add, remove) = (g.rows.clone(), add.to_vec(), remove.to_vec());
        self.store()
            .call(move |c| {
                let tx = c.transaction()?;
                for id in ids {
                    let kw: String = tx.query_row("SELECT keywords FROM messages WHERE id=?1", params![id], |r| r.get(0))?;
                    let mut v: Vec<String> = serde_json::from_str(&kw).unwrap_or_default();
                    v.retain(|k| !remove.iter().any(|r| r.eq_ignore_ascii_case(k)));
                    for a in &add {
                        if !v.iter().any(|k| k.eq_ignore_ascii_case(a)) {
                            v.push(a.clone());
                        }
                    }
                    tx.execute("UPDATE messages SET keywords=?2 WHERE id=?1", params![id, serde_json::to_string(&v)?])?;
                }
                tx.commit()?;
                Ok(())
            })
            .await
    }

    /// Gmail: change labels on each row, remembering exactly what changed for undo.
    async fn gmail_relabel(&self, g: &ApiGroup, add: &[&str], remove: &[&str]) -> Result<ApiUndo> {
        let api = self.api_client(&g.account).await?;
        let gm = Gmail { api: &api, store: self.store(), account_id: &g.account.id };
        let rows = self.remote_rows(&g.rows).await?;
        let mut items = Vec::new();
        // Group rows by the exact change they need, then one batchModify per group (Gmail::relabel_many).
        let mut groups: RelabelGroups = BTreeMap::new();
        for r in &rows {
            let current = current_label_ids(r);
            let has = |l: &str| current.iter().any(|c| c == l);
            // Only labels that really change, so undo restores the previous state exactly.
            let a: Vec<String> = add.iter().filter(|l| !has(l)).map(|l| l.to_string()).collect();
            let rm: Vec<String> = remove.iter().filter(|l| has(l)).map(|l| l.to_string()).collect();
            if a.is_empty() && rm.is_empty() {
                continue;
            }
            items.push((r.rowid, r.remote_id.clone(), a.clone(), rm.clone()));
            groups.entry((a, rm)).or_default().push((r.rowid, r.remote_id.clone(), current));
        }
        for ((a, rm), members) in &groups {
            gm.relabel_many(members, a, rm).await?;
        }
        Ok(ApiUndo::Gmail { account_id: g.account.id.clone(), items, untrash: false })
    }

    pub(crate) async fn api_archive(&self, g: &ApiGroup) -> Result<ApiUndo> {
        match g.backend {
            // Source: Gmail labels — archiving is removing INBOX.
            Backend::Gmail => self.gmail_relabel(g, &[], &["INBOX"]).await,
            _ => self.graph_move(g, "archive").await,
        }
    }

    /// Trash. Messages already in Trash: Graph deletes them for good (the UI asked first); Gmail cannot.
    pub(crate) async fn api_trash(&self, g: &ApiGroup) -> Result<Option<ApiUndo>> {
        let rows = self.remote_rows(&g.rows).await?;
        let in_trash = |r: &RemoteRow| r.folder_role == Some(FolderRole::Trash) || r.labels.iter().any(|(_, _, role)| *role == Some(FolderRole::Trash));
        let api = self.api_client(&g.account).await?;
        match g.backend {
            Backend::Gmail => {
                if rows.iter().any(in_trash) {
                    return Err(ApiError::invalid("ids", CAN_NOT_PURGE_GMAIL));
                }
                // Source: Gmail "Managing labels" — TRASH can be applied manually, so a bulk delete is one
                // batchModify adding TRASH per 1000 messages (same effect as users.messages.trash, which works
                // per message). Undo removes TRASH again, also in bulk.
                let _ = &api;
                Ok(Some(self.gmail_relabel(g, &["TRASH"], &[]).await?))
            }
            _ => {
                let gr = Graph { api: &api, store: self.store(), account_id: &g.account.id };
                let (purge, keep): (Vec<RemoteRow>, Vec<RemoteRow>) = rows.into_iter().partition(in_trash);
                for r in &purge {
                    gr.permanent_delete(&r.remote_id).await?;
                    let id = r.rowid;
                    self.store().call(move |c| crate::msgstore::delete_row(c, id)).await?;
                }
                if keep.is_empty() {
                    return Ok(None);
                }
                let g2 = ApiGroup { account: g.account.clone(), backend: g.backend, rows: keep.iter().map(|r| r.rowid).collect() };
                Ok(Some(self.graph_move(&g2, "deleteditems").await?))
            }
        }
    }

    pub(crate) async fn api_junk(&self, g: &ApiGroup, junk: bool) -> Result<ApiUndo> {
        match g.backend {
            // Source: Gmail labels — SPAM can be applied/removed by the user.
            Backend::Gmail if junk => self.gmail_relabel(g, &["SPAM"], &["INBOX"]).await,
            Backend::Gmail => self.gmail_relabel(g, &["INBOX"], &["SPAM"]).await,
            _ => self.graph_move(g, if junk { "junkemail" } else { "inbox" }).await,
        }
    }

    /// Move to a folder of the same account.
    pub(crate) async fn api_move(&self, g: &ApiGroup, dest_folder: i64) -> Result<ApiUndo> {
        let (_, remote, role, _) = self.store().call(move |c| store::folder_remote(c, dest_folder)).await?;
        let remote = remote.ok_or_else(|| ApiError::not_found("The destination folder on the server"))?;
        match g.backend {
            Backend::Gmail => match role {
                Some(FolderRole::All) => self.gmail_relabel(g, &[], &["INBOX"]).await,
                Some(FolderRole::Trash) => self.api_trash(g).await?.ok_or_else(|| ApiError::invalid("folderId", CAN_NOT_PURGE_GMAIL)),
                Some(FolderRole::Junk) => self.api_junk(g, true).await,
                Some(FolderRole::Inbox) => self.gmail_relabel(g, &["INBOX"], &["SPAM"]).await,
                // "Move to" a label: add it and take the message out of the Inbox, as Gmail's own Move to does.
                _ => self.gmail_relabel(g, &[remote.as_str()], &["INBOX"]).await,
            },
            _ => self.graph_move(g, &remote).await,
        }
    }

    pub(crate) async fn api_copy(&self, g: &ApiGroup, dest_folder: i64) -> Result<()> {
        let (_, remote, _, _) = self.store().call(move |c| store::folder_remote(c, dest_folder)).await?;
        let remote = remote.ok_or_else(|| ApiError::not_found("The destination folder on the server"))?;
        match g.backend {
            // A Gmail "copy" is one more label on the same message.
            Backend::Gmail => self.gmail_relabel(g, &[remote.as_str()], &[]).await.map(|_| ()),
            _ => {
                let api = self.api_client(&g.account).await?;
                let gr = Graph { api: &api, store: self.store(), account_id: &g.account.id };
                for r in self.remote_rows(&g.rows).await? {
                    gr.copy_to(&r.remote_id, &remote).await?;
                }
                self.sync_now(Some(g.account.id.clone())).await
            }
        }
    }

    async fn graph_move(&self, g: &ApiGroup, dest: &str) -> Result<ApiUndo> {
        let api = self.api_client(&g.account).await?;
        let gr = Graph { api: &api, store: self.store(), account_id: &g.account.id };
        let rows = self.remote_rows(&g.rows).await?;
        let aid = g.account.id.clone();
        // A well-known name ("archive") resolves through the stored roles; a folder id is used as is.
        let dest_local: Option<i64> = {
            let (aid, dest) = (aid.clone(), dest.to_string());
            self.store()
                .call(move |c| {
                    if let Some(id) = store::folder_by_remote(c, &aid, &dest)? {
                        return Ok(Some(id));
                    }
                    let role = crate::providers::graph::WELL_KNOWN.iter().find(|(wk, _)| *wk == dest).map(|(_, r)| *r);
                    Ok(match role {
                        Some(r) => store::role_folder(c, &aid, r)?.map(|(id, _)| id),
                        None => None,
                    })
                })
                .await?
        };
        let mut items = Vec::new();
        for r in &rows {
            let Some(orig) = r.folder_remote.clone() else { continue };
            let new_id = gr.move_to(&r.remote_id, dest).await?;
            let (row, nid, dl) = (r.rowid, new_id.clone(), dest_local);
            self.store()
                .call(move |c| {
                    c.execute("UPDATE messages SET remote_id=?2 WHERE id=?1", params![row, nid])?;
                    if let Some(f) = dl {
                        let uid: i64 = c.query_row("SELECT COALESCE(MAX(uid),0)+1 FROM messages WHERE folder_id=?1", [f], |x| x.get(0))?;
                        c.execute("UPDATE messages SET folder_id=?2, uid=?3 WHERE id=?1", params![row, f, uid])?;
                    }
                    Ok(())
                })
                .await?;
            items.push((r.rowid, new_id, orig));
        }
        if dest_local.is_none() {
            self.sync_now(Some(aid.clone())).await?;
        }
        Ok(ApiUndo::Graph { account_id: aid, items })
    }

    pub(crate) async fn api_undo(&self, u: ApiUndo) -> Result<Vec<String>> {
        match u {
            ApiUndo::Gmail { account_id, items, untrash } => {
                let acct = self.account(&account_id).await?;
                let api = self.api_client(&acct).await?;
                let gm = Gmail { api: &api, store: self.store(), account_id: &acct.id };
                if untrash {
                    // Undo records written before bulk delete existed (untrash per message).
                    for (row, rid, _, _) in &items {
                        gm.untrash(*row, rid).await?;
                    }
                } else {
                    // Reverse each recorded change, grouped like gmail_relabel so undo is bulk as well.
                    let rows = self.remote_rows(&items.iter().map(|(r, ..)| *r).collect::<Vec<_>>()).await?;
                    let mut groups: RelabelGroups = BTreeMap::new();
                    for (row, rid, added, removed) in &items {
                        let current = rows.iter().find(|r| r.rowid == *row).map(current_label_ids).unwrap_or_default();
                        groups.entry((removed.clone(), added.clone())).or_default().push((*row, rid.clone(), current));
                    }
                    for ((add, remove), members) in &groups {
                        gm.relabel_many(members, add, remove).await?;
                    }
                }
                self.account_folder_ids(&account_id).await
            }
            ApiUndo::Graph { account_id, items } => {
                let acct = self.account(&account_id).await?;
                for (row, _, orig) in items {
                    let one = ApiGroup { account: acct.clone(), backend: Backend::Graph, rows: vec![row] };
                    self.graph_move(&one, &orig).await?;
                }
                self.account_folder_ids(&account_id).await
            }
        }
    }

    /// Cross-account copy INTO an API account. Graph has no way to file an ordinary (non-draft) message from MIME
    /// (POST …/messages with MIME always creates a draft — Source: Create message), so that direction is refused
    /// with the reason instead of leaving drafts behind.
    pub(crate) async fn api_receive(&self, dest: &Folder, raw: &[u8], seen: bool) -> Result<()> {
        let acct = self.account(&dest.account_id).await?;
        match backend(&acct) {
            Backend::Gmail => {
                let fid: i64 = dest.id.parse().map_err(|_| ApiError::not_found("The folder"))?;
                let (_, remote, role, _) = self.store().call(move |c| store::folder_remote(c, fid)).await?;
                let mut labels: Vec<String> = match (role, remote) {
                    (Some(FolderRole::All), _) | (_, None) => vec![],
                    (_, Some(r)) => vec![r],
                };
                if !seen {
                    labels.push("UNREAD".into());
                }
                let api = self.api_client(&acct).await?;
                Gmail { api: &api, store: self.store(), account_id: &acct.id }.insert(raw, &labels).await?;
                self.sync_now(Some(acct.id.clone())).await
            }
            Backend::Graph => Err(ApiError::invalid(
                "folderId",
                "Microsoft 365 can't receive messages copied from another account — Outlook files them as drafts. Forward them instead.",
            )),
            Backend::Imap => Err(ApiError::internal("not an API account")),
        }
    }

    // ------------------------------------------------------------------ folders

    pub(crate) async fn api_folder_create(&self, acct: &Account, parent: Option<i64>, name: &str) -> Result<i64> {
        let api = self.api_client(acct).await?;
        let parent_info = match parent {
            Some(p) => Some(self.store().call(move |c| store::folder_remote(c, p)).await?),
            None => None,
        };
        let remote = match backend(acct) {
            Backend::Gmail => {
                let gm = Gmail { api: &api, store: self.store(), account_id: &acct.id };
                // A nested Gmail label is named "Parent/Child" (the delimiter Gmail reports for labels).
                let full = match &parent_info {
                    Some((_, _, None, path)) => format!("{path}/{name}"),
                    _ => name.to_string(),
                };
                let id = gm.create_label(&full).await?;
                gm.refresh_labels().await?;
                id
            }
            _ => {
                let gr = Graph { api: &api, store: self.store(), account_id: &acct.id };
                let id = gr.create_folder(parent_info.as_ref().and_then(|p| p.1.as_deref()), name).await?;
                gr.sync_folders().await?;
                id
            }
        };
        let aid = acct.id.clone();
        self.store()
            .call(move |c| store::folder_by_remote(c, &aid, &remote)?.ok_or_else(|| ApiError::not_found("The new folder")))
            .await
    }

    pub(crate) async fn api_folder_rename(&self, acct: &Account, folder: &Folder, name: &str) -> Result<()> {
        let api = self.api_client(acct).await?;
        let fid: i64 = folder.id.parse().map_err(|_| ApiError::not_found("The folder"))?;
        let (_, remote, role, path) = self.store().call(move |c| store::folder_remote(c, fid)).await?;
        let remote = remote.ok_or_else(|| ApiError::not_found("The folder on the server"))?;
        match backend(acct) {
            Backend::Gmail => {
                if role.is_some() {
                    return Err(ApiError::invalid("name", "Gmail's system folders can't be renamed."));
                }
                let gm = Gmail { api: &api, store: self.store(), account_id: &acct.id };
                let new_full = match path.rsplit_once('/') {
                    Some((p, _)) => format!("{p}/{name}"),
                    None => name.to_string(),
                };
                // Children carry the parent's name as a prefix, so they are renamed too.
                let (aid, old_prefix) = (acct.id.clone(), format!("{path}/"));
                let children: Vec<(String, String)> = self
                    .store()
                    .call(move |c| {
                        let mut st = c.prepare("SELECT remote_id, path FROM folders WHERE account_id=?1 AND substr(path,1,length(?2))=?2 AND remote_id IS NOT NULL")?;
                        let v = st.query_map(params![aid, old_prefix], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                        Ok(v)
                    })
                    .await?;
                gm.rename_label(&remote, &new_full).await?;
                for (rid, p) in children {
                    gm.rename_label(&rid, &format!("{new_full}{}", &p[path.len()..])).await?;
                }
                gm.refresh_labels().await?;
            }
            _ => {
                let gr = Graph { api: &api, store: self.store(), account_id: &acct.id };
                gr.rename_folder(&remote, name).await?;
                gr.sync_folders().await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn api_folder_delete(&self, acct: &Account, folder: &Folder) -> Result<()> {
        let api = self.api_client(acct).await?;
        let fid: i64 = folder.id.parse().map_err(|_| ApiError::not_found("The folder"))?;
        let (_, remote, role, _) = self.store().call(move |c| store::folder_remote(c, fid)).await?;
        let remote = remote.ok_or_else(|| ApiError::not_found("The folder on the server"))?;
        match backend(acct) {
            Backend::Gmail => {
                if role.is_some() {
                    return Err(ApiError::invalid("folder", "Gmail's system folders can't be deleted."));
                }
                let gm = Gmail { api: &api, store: self.store(), account_id: &acct.id };
                gm.delete_label(&remote).await?;
                gm.refresh_labels().await?;
            }
            _ => {
                let gr = Graph { api: &api, store: self.store(), account_id: &acct.id };
                gr.delete_folder(&remote).await?;
                gr.sync_folders().await?;
            }
        }
        Ok(())
    }

    /// Empty Trash / Junk on an API account: Outlook — every message in the folder on the server erased for good
    /// (permanentDelete, a few at a time); Gmail — refused, said why (CAN_NOT_EMPTY_GMAIL). → how many were erased.
    pub(crate) async fn api_folder_empty(&self, acct: &Account, folder_id: i64, progress: &(dyn Fn(usize, usize) + Send + Sync)) -> Result<usize> {
        if backend(acct) == Backend::Gmail {
            return Err(ApiError::invalid("id", CAN_NOT_EMPTY_GMAIL));
        }
        let (_, remote, _, _) = self.store().call(move |c| store::folder_remote(c, folder_id)).await?;
        let remote = remote.ok_or_else(|| ApiError::not_found("The folder on the server"))?;
        let api = self.api_client(acct).await?;
        let gr = Graph { api: &api, store: self.store(), account_id: &acct.id };
        let ids = gr.folder_message_ids(&remote).await?;
        let n = ids.len();
        let (mut done, mut failed) = (0usize, 0usize);
        progress(0, n);
        // 50 at a time, so the progress moves and a long empty can be followed.
        for chunk in ids.chunks(50) {
            let results = crate::providers::bounded(chunk.to_vec(), 4, |rid| {
                let gr = &gr;
                async move { gr.permanent_delete(&rid).await }
            })
            .await;
            failed += results.iter().filter(|r| r.is_err()).count();
            done += chunk.len();
            progress(done, n);
        }
        if failed > 0 {
            return Err(ApiError::internal(format!("{} of {n} messages could not be erased just now — try Empty again.", failed)));
        }
        Ok(n)
    }

    pub(crate) async fn api_folder_mark_read(&self, acct: &Account, folder_id: i64) -> Result<()> {
        let rows: Vec<(i64, String)> = self
            .store()
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT m.id, m.remote_id FROM messages m WHERE m.seen=0 AND m.remote_id IS NOT NULL
                       AND (m.folder_id=?1 OR m.id IN (SELECT message_rowid FROM message_labels WHERE folder_id=?1))",
                )?;
                let v = st.query_map(params![folder_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await?;
        let api = self.api_client(acct).await?;
        match backend(acct) {
            Backend::Gmail => {
                let ids: Vec<String> = rows.iter().map(|(_, r)| r.clone()).collect();
                Gmail { api: &api, store: self.store(), account_id: &acct.id }.batch_modify(&ids, &[], &["UNREAD".to_string()]).await?;
            }
            _ => {
                let gr = Graph { api: &api, store: self.store(), account_id: &acct.id };
                // Graph has no "mark folder read"; each message is updated (bounded so throttling is not provoked).
                let results = crate::providers::bounded(rows.clone(), 4, |(_, rid)| {
                    let gr = &gr;
                    async move { gr.patch(&rid, serde_json::json!({ "isRead": true })).await }
                })
                .await;
                for r in results {
                    r?;
                }
            }
        }
        let ids: Vec<i64> = rows.into_iter().map(|(i, _)| i).collect();
        self.store()
            .call(move |c| {
                for i in ids {
                    c.execute("UPDATE messages SET seen=1 WHERE id=?1", params![i])?;
                }
                Ok(())
            })
            .await
    }

    // ------------------------------------------------------------------ drafts and sending

    /// Save a draft's server copy. Returns the new server id (Gmail draft id / Graph message id).
    pub(crate) async fn api_save_draft(&self, acct: &Account, old_remote: Option<String>, raw: &[u8], reply_to_row: Option<i64>) -> Result<String> {
        let api = self.api_client(acct).await?;
        match backend(acct) {
            Backend::Gmail => {
                let thread = self.gmail_thread_for(reply_to_row).await?;
                Gmail { api: &api, store: self.store(), account_id: &acct.id }.save_draft(old_remote.as_deref(), raw, thread.as_deref()).await
            }
            _ => {
                // Graph can't replace a draft's MIME in place: create the new version, then delete the old one.
                let gr = Graph { api: &api, store: self.store(), account_id: &acct.id };
                let id = gr.create_draft(raw).await?;
                if let Some(old) = old_remote {
                    gr.delete(&old).await?;
                }
                Ok(id)
            }
        }
    }

    pub(crate) async fn api_delete_draft(&self, acct: &Account, remote: &str) -> Result<()> {
        let api = self.api_client(acct).await?;
        match backend(acct) {
            Backend::Gmail => Gmail { api: &api, store: self.store(), account_id: &acct.id }.delete_draft(remote).await,
            _ => Graph { api: &api, store: self.store(), account_id: &acct.id }.delete(remote).await,
        }
    }

    /// Server draft id for a draft MESSAGE row (Gmail: look the draft up by message id; Graph: the message id).
    pub(crate) async fn api_draft_id_for_row(&self, acct: &Account, rowid: i64) -> Result<Option<String>> {
        let r = self.remote_rows(&[rowid]).await?.remove(0);
        match backend(acct) {
            Backend::Gmail => {
                let api = self.api_client(acct).await?;
                Gmail { api: &api, store: self.store(), account_id: &acct.id }.find_draft(&r.remote_id).await
            }
            _ => Ok(Some(r.remote_id)),
        }
    }

    async fn gmail_thread_for(&self, reply_to_row: Option<i64>) -> Result<Option<String>> {
        let Some(row) = reply_to_row else { return Ok(None) };
        let tid: Option<String> = self
            .store()
            .call(move |c| Ok(rusqlite::OptionalExtension::optional(c.query_row("SELECT thread_id FROM messages WHERE id=?1", params![row], |r| r.get::<_, String>(0)))?))
            .await?;
        Ok(tid.as_deref().and_then(gmail::thread_of).map(str::to_string))
    }

    /// Send through the provider. Gmail and Graph both file the message in Sent themselves.
    pub(crate) async fn api_send(&self, acct: &Account, raw: &[u8], reply_to_row: Option<i64>) -> Result<()> {
        let api = self.api_client(acct).await?;
        match backend(acct) {
            Backend::Gmail => {
                let thread = self.gmail_thread_for(reply_to_row).await?;
                Gmail { api: &api, store: self.store(), account_id: &acct.id }.send(raw, thread.as_deref()).await.map(|_| ())
            }
            _ => Graph { api: &api, store: self.store(), account_id: &acct.id }.send(raw).await,
        }
    }

    /// account_test for an OAuth account: one authenticated call proves sign-in and API access.
    pub(crate) async fn api_test(&self, cfg: &ServerConfig) -> Result<()> {
        let token = oauth::access_token(&self.inner.http, self.inner.secrets.as_ref(), &cfg.username).await?;
        match crate::providers::backend_for_auth(cfg.auth) {
            Backend::Gmail => {
                // Source: users.getProfile.
                ApiClient::new(&self.inner.http, token, &self.inner.api_bases.gmail, false, &cfg.username).get_json::<serde_json::Value>("/users/me/profile", &[]).await?;
            }
            _ => {
                // Source: Graph inbox via its well-known name (needs Mail.ReadWrite, which WolfMail asks for).
                ApiClient::new(&self.inner.http, token, &self.inner.api_bases.graph, true, &cfg.username)
                    .get_json::<serde_json::Value>("/me/mailFolders/inbox", &[("$select", "id".to_string())])
                    .await?;
            }
        }
        Ok(())
    }

    /// One sync pass for an API account: (more backlog, changed folder ids, new unread inbox rows).
    pub(crate) async fn api_pass(&self, acct: &Account) -> Result<(bool, Vec<String>, Vec<i64>)> {
        let api = self.api_client(acct).await?;
        match backend(acct) {
            Backend::Gmail => {
                let out = Gmail { api: &api, store: self.store(), account_id: &acct.id }.sync_pass(acct.body_days).await?;
                let changed = if out.changed { self.account_folder_ids(&acct.id).await? } else { Vec::new() };
                Ok((out.more, changed, out.new_inbox))
            }
            _ => {
                let out = Graph { api: &api, store: self.store(), account_id: &acct.id }.sync_pass(acct.body_days).await?;
                Ok((out.more, out.changed.into_iter().map(|i| i.to_string()).collect(), out.new_inbox))
            }
        }
    }
}

/// Rows grouped by the label change they need: (labels to add, labels to remove) → (rowid, remote id, current labels).
type RelabelGroups = BTreeMap<(Vec<String>, Vec<String>), Vec<(i64, String, Vec<String>)>>;

/// A Gmail row's current label ids as the API names them: its label folders plus UNREAD / STARRED from the flags.
fn current_label_ids(r: &crate::providers::store::RemoteRow) -> Vec<String> {
    let mut v: Vec<String> = r.labels.iter().map(|(_, rid, _)| rid.clone()).collect();
    if !r.seen {
        v.push("UNREAD".into());
    }
    if r.flagged {
        v.push("STARRED".into());
    }
    v
}
