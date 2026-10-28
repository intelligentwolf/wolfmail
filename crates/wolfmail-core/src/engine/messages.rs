//! Message commands: list, read, flags, tags, move/copy/archive/trash/junk + undo, attachments, search,
//! contact suggestions, unsubscribe.
// Source: docs/API.md commands messages_* / message_* / thread_get / attachment_* / search / contacts_suggest /
// unsubscribe, and the `undoable` event (token valid 10 s).
use super::accounts::folder_with_role;
use super::apiops::{ApiGroup, ApiUndo};
use super::{Engine, UndoEntry, UndoMove};
use crate::db::now_ms;
use crate::error::{ApiError, Result};
use crate::imap::sync::fetch_body;
use crate::mime;
use crate::model::*;
use crate::msgstore;
use rusqlite::{params, params_from_iter, OptionalExtension};
use std::collections::BTreeMap;

const UNDO_MS: i64 = 10_000;

struct Group {
    account_id: String,
    folder_id: i64,
    path: String,
    items: Vec<(i64, u32)>,
}

fn parse_ids(ids: &[String]) -> Result<Vec<i64>> {
    ids.iter().map(|s| s.parse::<i64>().map_err(|_| ApiError::not_found("A message"))).collect()
}

fn flag_name(f: &str) -> Result<&'static str> {
    // Source: RFC 3501 §2.3.2 system flags; RFC 5788 / IANA keyword registry `$Forwarded`.
    Ok(match f {
        "seen" => "\\Seen",
        "flagged" => "\\Flagged",
        "answered" => "\\Answered",
        "forwarded" => "$Forwarded",
        "draft" => "\\Draft",
        other => return Err(ApiError::invalid("flags", format!("Unknown flag “{other}”."))),
    })
}

fn valid_keyword(k: &str) -> bool {
    // RFC 3501 §9: atom — no ( ) { SP CTL % * " \ ]
    !k.is_empty() && !k.chars().any(|c| c.is_control() || " (){%*\"\\]".contains(c)) && !k.starts_with('\\')
}

impl Engine {
    async fn groups(&self, ids: &[String]) -> Result<Vec<Group>> {
        let ids = parse_ids(ids)?;
        self.store()
            .call(move |c| {
                let mut map: BTreeMap<i64, Group> = BTreeMap::new();
                for id in ids {
                    let (fid, path, uid, aid) = msgstore::locate(c, id)?;
                    map.entry(fid).or_insert_with(|| Group { account_id: aid, folder_id: fid, path, items: Vec::new() }).items.push((id, uid));
                }
                Ok(map.into_values().collect())
            })
            .await
    }

    pub async fn messages_list(&self, q: ViewQuery) -> Result<MessagePage> {
        self.store().call(move |c| msgstore::list_view(c, &q)).await
    }

    pub async fn thread_get(&self, thread_id: String, account_id: String) -> Result<Vec<MessageSummary>> {
        self.store().call(move |c| msgstore::thread_messages(c, &account_id, &thread_id)).await
    }

    /// Raw RFC 822 bytes, from the cache or the server.
    pub(crate) async fn raw_bytes(&self, id: i64) -> Result<Vec<u8>> {
        if let Some(raw) = self.store().call(move |c| msgstore::body_raw(c, id)).await? {
            return Ok(raw);
        }
        let remote: Option<String> = self
            .store()
            .call(move |c| Ok(c.query_row("SELECT remote_id FROM messages WHERE id=?1", params![id], |r| r.get(0)).optional()?.flatten()))
            .await?;
        if remote.is_some() {
            return self.api_raw(id).await;
        }
        let (_, path, uid, aid) = self.store().call(move |c| msgstore::locate(c, id)).await?;
        let store = self.store().clone();
        self.with_interactive(&aid, move |conn| {
            let (store, path) = (store.clone(), path.clone());
            Box::pin(async move { fetch_body(conn, &store, id, &path, uid).await })
        })
        .await
    }

    async fn allow_remote_for(&self, id: i64) -> Result<bool> {
        let s = self.settings_get().await?;
        Ok(match s.load_remote_content.as_str() {
            "always" => true,
            "contacts" => self
                .store()
                .call(move |c| {
                    Ok(c.query_row(
                        "SELECT COUNT(*) FROM contacts_seen cs JOIN messages m ON m.from_email=cs.email WHERE m.id=?1 AND cs.sent_to=1",
                        params![id],
                        |r| r.get::<_, i64>(0),
                    )? > 0)
                })
                .await?,
            _ => false,
        })
    }

    pub async fn message_get(&self, id: String) -> Result<MessageFull> {
        let rid: i64 = id.parse().map_err(|_| ApiError::not_found("The message"))?;
        let raw = self.raw_bytes(rid).await?;
        let allow = self.allow_remote_for(rid).await?;
        let raw2 = raw.clone();
        let mut full = self.store().call(move |c| msgstore::assemble_full(c, rid, &raw2, allow)).await?;
        full.invite = crate::pim::itip::invite_for(self, &raw).await;
        Ok(full)
    }

    pub async fn message_load_remote(&self, id: String) -> Result<MessageFull> {
        let rid: i64 = id.parse().map_err(|_| ApiError::not_found("The message"))?;
        let raw = self.raw_bytes(rid).await?;
        let raw2 = raw.clone();
        let mut full = self.store().call(move |c| msgstore::assemble_full(c, rid, &raw2, true)).await?;
        full.invite = crate::pim::itip::invite_for(self, &raw).await;
        Ok(full)
    }

    pub async fn message_raw(&self, id: String) -> Result<String> {
        let rid: i64 = id.parse().map_err(|_| ApiError::not_found("The message"))?;
        Ok(String::from_utf8_lossy(&self.raw_bytes(rid).await?).into_owned())
    }

    pub async fn messages_set_flags(&self, ids: Vec<String>, add: Vec<String>, remove: Vec<String>) -> Result<()> {
        let add_f: Vec<String> = add.iter().map(|f| flag_name(f).map(String::from)).collect::<Result<_>>()?;
        let rem_f: Vec<String> = remove.iter().map(|f| flag_name(f).map(String::from)).collect::<Result<_>>()?;
        let (ids, api) = self.api_partition(&ids).await?;
        let mut touched = Vec::new();
        for g in &api {
            self.api_set_flags(g, &add, &remove).await?;
            touched.extend(self.account_folder_ids(&g.account.id).await?);
        }
        let groups = self.groups(&ids).await?;
        for g in groups {
            let uids: Vec<u32> = g.items.iter().map(|(_, u)| *u).collect();
            let (path, a, r) = (g.path.clone(), add_f.clone(), rem_f.clone());
            self.with_interactive(&g.account_id, move |conn| {
                let (path, a, r, uids) = (path.clone(), a.clone(), r.clone(), uids.clone());
                Box::pin(async move { conn.store_flags(&path, &uids, &a, &r).await })
            })
            .await?;
            let rows: Vec<i64> = g.items.iter().map(|(id, _)| *id).collect();
            let (add, remove) = (add.clone(), remove.clone());
            self.store()
                .call(move |c| {
                    let tx = c.transaction()?;
                    for id in rows {
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
                .await?;
            touched.push(g.folder_id.to_string());
        }
        self.emit("mail://changed", serde_json::json!({ "folderIds": touched }));
        self.emit_counts().await;
        Ok(())
    }

    pub async fn messages_set_tags(&self, ids: Vec<String>, add: Vec<String>, remove: Vec<String>) -> Result<()> {
        if let Some(bad) = add.iter().chain(remove.iter()).find(|k| !valid_keyword(k)) {
            return Err(ApiError::invalid("tags", format!("“{bad}” cannot be used as a tag keyword.")));
        }
        let (ids, api) = self.api_partition(&ids).await?;
        let mut touched = Vec::new();
        for g in &api {
            self.api_set_tags(g, &add, &remove).await?;
            touched.extend(self.account_folder_ids(&g.account.id).await?);
        }
        let groups = self.groups(&ids).await?;
        for g in groups {
            let uids: Vec<u32> = g.items.iter().map(|(_, u)| *u).collect();
            let (path, a, r) = (g.path.clone(), add.clone(), remove.clone());
            self.with_interactive(&g.account_id, move |conn| {
                let (path, a, r, uids) = (path.clone(), a.clone(), r.clone(), uids.clone());
                Box::pin(async move { conn.store_flags(&path, &uids, &a, &r).await })
            })
            .await?;
            let rows: Vec<i64> = g.items.iter().map(|(id, _)| *id).collect();
            let (add, remove) = (add.clone(), remove.clone());
            self.store()
                .call(move |c| {
                    let tx = c.transaction()?;
                    for id in rows {
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
                .await?;
            touched.push(g.folder_id.to_string());
        }
        self.emit("mail://changed", serde_json::json!({ "folderIds": touched }));
        Ok(())
    }

    /// Move one folder's worth of messages on the server and mirror it locally. Returns what undo needs.
    async fn move_group(&self, g: &Group, to_folder: i64, to_path: &str) -> Result<UndoMove> {
        let uids: Vec<u32> = g.items.iter().map(|(_, u)| *u).collect();
        let rows: Vec<i64> = g.items.iter().map(|(id, _)| *id).collect();
        let msg_ids: Vec<String> = {
            let r = rows.clone();
            self.store()
                .call(move |c| {
                    let mut out = Vec::new();
                    for id in r {
                        if let Some(m) = c.query_row("SELECT message_id FROM messages WHERE id=?1", params![id], |x| x.get::<_, Option<String>>(0)).optional()?.flatten() {
                            out.push(m);
                        }
                    }
                    Ok(out)
                })
                .await?
        };
        let (from, to) = (g.path.clone(), to_path.to_string());
        let mapping = self
            .with_interactive(&g.account_id, move |conn| {
                let (from, to, uids) = (from.clone(), to.clone(), uids.clone());
                Box::pin(async move { conn.move_uids(&from, &uids, &to).await })
            })
            .await?;
        let items = g.items.clone();
        let new_uids: Vec<u32> = match &mapping {
            Some(m) => items.iter().filter_map(|(_, u)| m.iter().find(|(s, _)| s == u).map(|(_, d)| *d)).collect(),
            None => Vec::new(),
        };
        let map2 = mapping.clone();
        self.store()
            .call(move |c| {
                let tx = c.transaction()?;
                for (id, uid) in items {
                    let dest = map2.as_ref().and_then(|m| m.iter().find(|(s, _)| *s == uid).map(|(_, d)| *d));
                    match dest {
                        Some(d) => {
                            // A sync may already have fetched the moved copy; keep ours (it has the cached body).
                            if let Some(dup) = tx
                                .query_row("SELECT id FROM messages WHERE folder_id=?1 AND uid=?2", params![to_folder, d], |r| r.get::<_, i64>(0))
                                .optional()?
                            {
                                msgstore::delete_row(&tx, dup)?;
                            }
                            tx.execute("UPDATE messages SET folder_id=?2, uid=?3 WHERE id=?1", params![id, to_folder, d])?;
                        }
                        None => msgstore::delete_row(&tx, id)?,
                    }
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
        if mapping.is_none() {
            // Without COPYUID the destination copy arrives with the next sync of that folder.
            self.sync_now(Some(g.account_id.clone())).await?;
        }
        Ok(UndoMove {
            account_id: g.account_id.clone(),
            current_folder_id: to_folder,
            original_folder_id: g.folder_id,
            uids: new_uids,
            message_ids: msg_ids,
        })
    }

    async fn ensure_role_folder(&self, account_id: &str, role: FolderRole, default_name: &str) -> Result<(i64, String)> {
        let aid = account_id.to_string();
        if let Some(f) = self.store().call(move |c| folder_with_role(c, &aid, role)).await? {
            return Ok(f);
        }
        let f = self.folder_create(account_id.to_string(), None, default_name.to_string()).await?;
        let fid: i64 = f.id.parse().map_err(|_| ApiError::internal("folder id"))?;
        self.store()
            .call(move |c| {
                c.execute("UPDATE folders SET role=?2 WHERE id=?1", params![fid, role.as_str()])?;
                Ok(())
            })
            .await?;
        Ok((fid, f.path))
    }

    async fn move_with_undo(
        &self,
        groups: Vec<Group>,
        dest: impl Fn(&str) -> Option<(i64, String)>,
        label: &str,
        api: Vec<ApiUndo>,
        api_touched: Vec<String>,
    ) -> Result<()> {
        let mut moves = Vec::new();
        let mut touched = api_touched;
        for g in groups {
            let Some((to_fid, to_path)) = dest(&g.account_id) else { continue };
            if to_fid == g.folder_id {
                continue;
            }
            touched.push(g.folder_id.to_string());
            touched.push(to_fid.to_string());
            moves.push(self.move_group(&g, to_fid, &to_path).await?);
        }
        self.emit("mail://changed", serde_json::json!({ "folderIds": touched }));
        self.emit_counts().await;
        if !moves.is_empty() || !api.is_empty() {
            let token = uuid::Uuid::new_v4().to_string();
            let expires_at = now_ms() + UNDO_MS;
            {
                let mut u = self.inner.undo.lock();
                u.retain(|_, e| e.expires_at > now_ms());
                u.insert(token.clone(), UndoEntry { expires_at, moves, api });
            }
            self.emit("undoable", serde_json::json!({ "token": token, "label": label, "expiresAt": expires_at }));
        }
        Ok(())
    }

    pub async fn messages_move(&self, ids: Vec<String>, folder_id: String) -> Result<()> {
        let fid: i64 = folder_id.parse().map_err(|_| ApiError::not_found("The folder"))?;
        let dest = self.store().call(move |c| super::accounts::folder_by_id(c, fid)).await?;
        let n = ids.len();
        let (imap_ids, api) = self.api_partition(&ids).await?;
        let (same, other): (Vec<ApiGroup>, Vec<ApiGroup>) = api.into_iter().partition(|g| g.account.id == dest.account_id);
        let mut undo = Vec::new();
        let mut touched = Vec::new();
        for g in &same {
            undo.push(self.api_move(g, fid).await?);
            touched.extend(self.account_folder_ids(&g.account.id).await?);
        }
        let mut cross: Vec<i64> = other.into_iter().flat_map(|g| g.rows).collect();
        let groups = self.groups(&imap_ids).await?;
        let (local, remote): (Vec<Group>, Vec<Group>) = groups.into_iter().partition(|g| g.account_id == dest.account_id);
        cross.extend(remote.iter().flat_map(|g| g.items.iter().map(|(id, _)| *id)));
        if !cross.is_empty() {
            self.cross_account_rows(cross, &dest, true).await?;
        }
        let label = format!("Moved {n} message{} to {}", if n == 1 { "" } else { "s" }, dest.name);
        let (dfid, dpath) = (fid, dest.path.clone());
        self.move_with_undo(local, move |_| Some((dfid, dpath.clone())), &label, undo, touched).await
    }

    pub async fn messages_copy(&self, ids: Vec<String>, folder_id: String) -> Result<()> {
        let fid: i64 = folder_id.parse().map_err(|_| ApiError::not_found("The folder"))?;
        let dest = self.store().call(move |c| super::accounts::folder_by_id(c, fid)).await?;
        let (imap_ids, api) = self.api_partition(&ids).await?;
        let (same, other): (Vec<ApiGroup>, Vec<ApiGroup>) = api.into_iter().partition(|g| g.account.id == dest.account_id);
        for g in &same {
            self.api_copy(g, fid).await?;
        }
        let mut cross: Vec<i64> = other.into_iter().flat_map(|g| g.rows).collect();
        let groups = self.groups(&imap_ids).await?;
        let (groups, remote): (Vec<Group>, Vec<Group>) = groups.into_iter().partition(|g| g.account_id == dest.account_id);
        cross.extend(remote.iter().flat_map(|g| g.items.iter().map(|(id, _)| *id)));
        if !cross.is_empty() {
            self.cross_account_rows(cross, &dest, false).await?;
        }
        if groups.is_empty() {
            self.emit("mail://changed", serde_json::json!({ "folderIds": [dest.id] }));
            return Ok(());
        }
        for g in &groups {
            let uids: Vec<u32> = g.items.iter().map(|(_, u)| *u).collect();
            let (from, to) = (g.path.clone(), dest.path.clone());
            self.with_interactive(&g.account_id, move |conn| {
                let (from, to, uids) = (from.clone(), to.clone(), uids.clone());
                Box::pin(async move { conn.copy_uids(&from, &uids, &to).await.map(|_| ()) })
            })
            .await?;
        }
        self.sync_now(Some(dest.account_id.clone())).await?;
        Ok(())
    }

    /// Messages dragged to another account's folder: put the raw message there (IMAP APPEND / Gmail insert) and,
    /// when moving, remove the original (IMAP: deleted + expunged; API accounts: moved to Trash). There is no
    /// cross-server MOVE in IMAP or in the provider APIs.
    async fn cross_account_rows(&self, rows: Vec<i64>, dest: &Folder, delete_source: bool) -> Result<()> {
        let dest_acct = self.account(&dest.account_id).await?;
        let dest_api = crate::providers::backend(&dest_acct) != crate::providers::Backend::Imap;
        for id in &rows {
            let raw = self.raw_bytes(*id).await?;
            let seen = self
                .store()
                .call({
                    let id = *id;
                    move |c| Ok(c.query_row("SELECT seen FROM messages WHERE id=?1", params![id], |r| r.get::<_, bool>(0))?)
                })
                .await?;
            if dest_api {
                self.api_receive(dest, &raw, seen).await?;
            } else {
                let path = dest.path.clone();
                self.with_interactive(&dest.account_id, move |conn| {
                    let (path, raw) = (path.clone(), raw.clone());
                    Box::pin(async move { conn.append_msg(&path, if seen { "\\Seen" } else { "" }, &raw).await.map(|_| ()) })
                })
                .await?;
            }
        }
        if delete_source {
            let ids: Vec<String> = rows.iter().map(|i| i.to_string()).collect();
            let (imap_ids, api) = self.api_partition(&ids).await?;
            for g in &api {
                self.api_trash(g).await?;
            }
            for g in self.groups(&imap_ids).await? {
                let uids: Vec<u32> = g.items.iter().map(|(_, u)| *u).collect();
                let path = g.path.clone();
                self.with_interactive(&g.account_id, move |conn| {
                    let (path, uids) = (path.clone(), uids.clone());
                    Box::pin(async move { conn.delete_uids(&path, &uids).await })
                })
                .await?;
                let rows: Vec<i64> = g.items.iter().map(|(id, _)| *id).collect();
                self.store()
                    .call(move |c| {
                        for id in rows {
                            msgstore::delete_row(c, id)?;
                        }
                        Ok(())
                    })
                    .await?;
            }
        }
        if !dest_api {
            self.sync_now(Some(dest.account_id.clone())).await?;
        }
        self.emit("mail://changed", serde_json::json!({ "folderIds": [dest.id] }));
        Ok(())
    }

    pub async fn messages_archive(&self, ids: Vec<String>) -> Result<()> {
        let n = ids.len();
        let (ids, api) = self.api_partition(&ids).await?;
        let (mut undo, mut touched) = (Vec::new(), Vec::new());
        for g in &api {
            undo.push(self.api_archive(g).await?);
            touched.extend(self.account_folder_ids(&g.account.id).await?);
        }
        let groups = self.groups(&ids).await?;
        let mut dests = BTreeMap::new();
        for g in &groups {
            if !dests.contains_key(&g.account_id) {
                dests.insert(g.account_id.clone(), self.ensure_role_folder(&g.account_id, FolderRole::Archive, "Archive").await?);
            }
        }
        self.move_with_undo(groups, move |a| dests.get(a).cloned(), &format!("Archived {n} message{}", if n == 1 { "" } else { "s" }), undo, touched)
            .await
    }

    pub async fn messages_trash(&self, ids: Vec<String>) -> Result<()> {
        let n = ids.len();
        let (ids, api) = self.api_partition(&ids).await?;
        let (mut undo, mut touched) = (Vec::new(), Vec::new());
        for g in &api {
            if let Some(u) = self.api_trash(g).await? {
                undo.push(u);
            }
            touched.extend(self.account_folder_ids(&g.account.id).await?);
        }
        let groups = self.groups(&ids).await?;
        let mut to_move = Vec::new();
        let mut dests = BTreeMap::new();
        for g in groups {
            let aid = g.account_id.clone();
            let trash = self.ensure_role_folder(&aid, FolderRole::Trash, "Trash").await?;
            if trash.0 == g.folder_id {
                // Already in Trash: delete for good (no undo — the UI asks first).
                let uids: Vec<u32> = g.items.iter().map(|(_, u)| *u).collect();
                let path = g.path.clone();
                self.with_interactive(&aid, move |conn| {
                    let (path, uids) = (path.clone(), uids.clone());
                    Box::pin(async move { conn.delete_uids(&path, &uids).await })
                })
                .await?;
                let rows: Vec<i64> = g.items.iter().map(|(id, _)| *id).collect();
                self.store()
                    .call(move |c| {
                        for id in rows {
                            msgstore::delete_row(c, id)?;
                        }
                        Ok(())
                    })
                    .await?;
                self.emit("mail://changed", serde_json::json!({ "folderIds": [g.folder_id.to_string()] }));
            } else {
                dests.insert(aid, trash);
                to_move.push(g);
            }
        }
        self.move_with_undo(to_move, move |a| dests.get(a).cloned(), &format!("Deleted {n} message{}", if n == 1 { "" } else { "s" }), undo, touched)
            .await
    }

    pub async fn messages_junk(&self, ids: Vec<String>, junk: bool) -> Result<()> {
        // Source: RFC 5788 registry — $Junk / $NotJunk keywords tell server-side filters what the user decided.
        let (add, remove) = if junk { ("$Junk", "$NotJunk") } else { ("$NotJunk", "$Junk") };
        let n = ids.len();
        let (ids, api) = self.api_partition(&ids).await?;
        let (mut undo, mut touched) = (Vec::new(), Vec::new());
        for g in &api {
            undo.push(self.api_junk(g, junk).await?);
            touched.extend(self.account_folder_ids(&g.account.id).await?);
        }
        let groups = self.groups(&ids).await?;
        for g in &groups {
            let uids: Vec<u32> = g.items.iter().map(|(_, u)| *u).collect();
            let path = g.path.clone();
            let _ = self
                .with_interactive(&g.account_id, move |conn| {
                    let (path, uids) = (path.clone(), uids.clone());
                    Box::pin(async move { conn.store_flags(&path, &uids, &[add.to_string()], &[remove.to_string()]).await })
                })
                .await;
        }
        let mut dests = BTreeMap::new();
        for g in &groups {
            if !dests.contains_key(&g.account_id) {
                let d = if junk {
                    self.ensure_role_folder(&g.account_id, FolderRole::Junk, "Junk").await?
                } else {
                    let aid = g.account_id.clone();
                    self.store()
                        .call(move |c| folder_with_role(c, &aid, FolderRole::Inbox))
                        .await?
                        .ok_or_else(|| ApiError::not_found("The Inbox"))?
                };
                dests.insert(g.account_id.clone(), d);
            }
        }
        let label = if junk { format!("Marked {n} as junk") } else { format!("Moved {n} back to Inbox") };
        self.move_with_undo(groups, move |a| dests.get(a).cloned(), &label, undo, touched).await
    }

    pub async fn messages_undo(&self, token: String) -> Result<()> {
        let entry = self.inner.undo.lock().remove(&token);
        let Some(entry) = entry else {
            return Err(ApiError::invalid("token", "It is too late to undo that."));
        };
        if entry.expires_at < now_ms() {
            return Err(ApiError::invalid("token", "It is too late to undo that."));
        }
        let mut touched = Vec::new();
        for u in entry.api {
            touched.extend(self.api_undo(u).await?);
        }
        for m in entry.moves {
            let (cur, orig) = (m.current_folder_id, m.original_folder_id);
            let (cur_path, orig_path): (String, String) = self
                .store()
                .call(move |c| {
                    let a: String = c.query_row("SELECT path FROM folders WHERE id=?1", params![cur], |r| r.get(0))?;
                    let b: String = c.query_row("SELECT path FROM folders WHERE id=?1", params![orig], |r| r.get(0))?;
                    Ok((a, b))
                })
                .await?;
            let mut uids = m.uids.clone();
            if uids.is_empty() {
                for mid in &m.message_ids {
                    let (p, mid) = (cur_path.clone(), mid.clone());
                    if let Some(u) = self
                        .with_interactive(&m.account_id, move |conn| {
                            let (p, mid) = (p.clone(), mid.clone());
                            Box::pin(async move { conn.find_uid_by_message_id(&p, &mid).await })
                        })
                        .await?
                    {
                        uids.push(u);
                    }
                }
            }
            if uids.is_empty() {
                continue;
            }
            let rows: Vec<(i64, u32)> = {
                let u2 = uids.clone();
                self.store()
                    .call(move |c| {
                        let mut v = Vec::new();
                        for u in u2 {
                            if let Some(id) = c.query_row("SELECT id FROM messages WHERE folder_id=?1 AND uid=?2", params![cur, u], |r| r.get(0)).optional()? {
                                v.push((id, u));
                            }
                        }
                        Ok(v)
                    })
                    .await?
            };
            let g = Group {
                account_id: m.account_id.clone(),
                folder_id: cur,
                path: cur_path.clone(),
                // No local rows (not synced yet): -1 rowids make the local mirror step a no-op; the server move still runs.
                items: if rows.is_empty() { uids.iter().map(|u| (-1, *u)).collect() } else { rows },
            };
            self.move_group(&g, orig, &orig_path).await?;
            touched.push(cur.to_string());
            touched.push(orig.to_string());
        }
        self.emit("mail://changed", serde_json::json!({ "folderIds": touched }));
        self.emit_counts().await;
        Ok(())
    }

    async fn attachment(&self, message_id: &str, attachment_id: &str) -> Result<(String, String, Vec<u8>)> {
        let rid: i64 = message_id.parse().map_err(|_| ApiError::not_found("The message"))?;
        let raw = self.raw_bytes(rid).await?;
        mime::attachment_bytes(&raw, attachment_id).ok_or_else(|| ApiError::not_found("The attachment"))
    }

    pub async fn attachment_save(&self, message_id: String, attachment_id: String, path: String) -> Result<()> {
        let (_, _, bytes) = self.attachment(&message_id, &attachment_id).await?;
        tokio::fs::write(&path, bytes).await.map_err(|e| ApiError::storage(format!("Could not save to {path}: {e}")))
    }

    pub async fn attachment_open(&self, message_id: String, attachment_id: String) -> Result<()> {
        let (name, _, bytes) = self.attachment(&message_id, &attachment_id).await?;
        // A sanitised file name inside a per-message cache directory; never a path the sender controls.
        let safe: String = name
            .chars()
            .map(|c| if c.is_alphanumeric() || " ._-()".contains(c) { c } else { '_' })
            .collect::<String>()
            .trim_start_matches('.')
            .to_string();
        let safe = if safe.is_empty() { "attachment".to_string() } else { safe };
        let dir = self.inner.cfg.cache_dir.join("attachments").join(&message_id);
        tokio::fs::create_dir_all(&dir).await?;
        let file = dir.join(safe);
        tokio::fs::write(&file, bytes).await?;
        (self.inner.opener)(&file.to_string_lossy())
    }

    pub async fn search(&self, query: String, account_id: Option<String>, limit: i64) -> Result<Vec<SearchHit>> {
        let parsed = crate::search::parse_query(&query);
        let (sql, mut ps) = parsed.to_sql();
        if sql.is_empty() {
            return Ok(Vec::new());
        }
        let mut where_sql = format!("{sql} AND m.account_id IN (SELECT id FROM accounts WHERE enabled=1)");
        if let Some(a) = account_id {
            where_sql.push_str(" AND m.account_id = ?");
            ps.push(rusqlite::types::Value::Text(a));
        }
        let limit = limit.clamp(1, 500);
        self.store()
            .call(move |c| {
                let mut st = c.prepare(&format!(
                    // IMAP "All Mail" copies are duplicates of other folders' rows; a Gmail-API account keeps its
                    // single row per message in its All Mail folder (remote_id set), so that one must be searched.
                    "SELECT {} FROM messages m JOIN folders f ON f.id=m.folder_id WHERE {where_sql}
                       AND (COALESCE(f.role,'') <> 'all' OR f.remote_id IS NOT NULL)
                     ORDER BY m.date DESC LIMIT {limit}",
                    msgstore::SUMMARY_COLS
                ))?;
                let v = st
                    .query_map(params_from_iter(ps.iter()), |r| {
                        let s = msgstore::row_to_summary(r, 1, 0)?;
                        let highlight = if s.snippet.is_empty() { s.subject.clone() } else { s.snippet.clone() };
                        Ok(SearchHit { summary: s, highlight })
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await
    }

    pub async fn contacts_suggest(&self, prefix: String, limit: i64) -> Result<Vec<Address>> {
        let p = prefix.trim().to_lowercase();
        if p.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 50);
        let prefix_for_book = p.clone();
        let seen = self
            .store()
            .call(move |c| {
                let like = format!("{}%", p.replace('%', "\\%").replace('_', "\\_"));
                let any = format!("%{}%", p.replace('%', "\\%").replace('_', "\\_"));
                let mut st = c.prepare(
                    "SELECT email, name FROM contacts_seen
                     WHERE email LIKE ?1 ESCAPE '\\' OR lower(COALESCE(name,'')) LIKE ?2 ESCAPE '\\'
                     ORDER BY sent_to DESC, count DESC, last_seen DESC LIMIT ?3",
                )?;
                let v = st
                    .query_map(params![like, any, limit], |r| Ok(Address { email: r.get(0)?, name: r.get(1)? }))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await?;
        // Address-book contacts (calendar & contacts) are blended in: pim/commands.rs pim_suggest_merge.
        Ok(self.pim_suggest_merge(seen, &prefix_for_book, limit).await)
    }

    pub async fn unsubscribe(&self, message_id: String) -> Result<String> {
        let rid: i64 = message_id.parse().map_err(|_| ApiError::not_found("The message"))?;
        let (lu, account_id): (Option<String>, String) = self
            .store()
            .call(move |c| Ok(c.query_row("SELECT list_unsub_json, account_id FROM messages WHERE id=?1", params![rid], |r| Ok((r.get(0)?, r.get(1)?)))?))
            .await?;
        let lu: ListUnsubscribe = lu
            .and_then(|s| serde_json::from_str(&s).ok())
            .ok_or_else(|| ApiError::invalid("messageId", "This message has no unsubscribe link."))?;
        if lu.one_click {
            if let Some(url) = &lu.http {
                // Source: RFC 8058 §3.1 — POST with body "List-Unsubscribe=One-Click", no cookies, no auth.
                let resp = self
                    .inner
                    .http
                    .post(url)
                    .header("Content-Type", "application/x-www-form-urlencoded")
                    .body("List-Unsubscribe=One-Click")
                    .send()
                    .await?;
                if resp.status().is_success() || resp.status().is_redirection() {
                    return Ok("oneClick".into());
                }
                return Err(ApiError::server(format!("The sender's unsubscribe link answered {}.", resp.status())));
            }
        }
        if let Some(mailto) = &lu.mailto {
            let u = url::Url::parse(mailto).map_err(|_| ApiError::invalid("messageId", "The unsubscribe address is malformed."))?;
            let to = u.path().to_string();
            let mut subject = "unsubscribe".to_string();
            let mut body = "unsubscribe".to_string();
            for (k, v) in u.query_pairs() {
                match k.to_ascii_lowercase().as_str() {
                    "subject" => subject = v.into_owned(),
                    "body" => body = v.into_owned(),
                    _ => {}
                }
            }
            let acct = self.account(&account_id).await?;
            let ident = acct.identities.iter().find(|i| i.is_default).or(acct.identities.first()).ok_or_else(|| ApiError::not_found("An identity"))?;
            let draft = DraftInput {
                id: None,
                identity_id: ident.id.clone(),
                to: vec![Address { name: None, email: to }],
                cc: vec![],
                bcc: vec![],
                subject,
                html: String::new(),
                text: body,
                in_reply_to: None,
                references: vec![],
                reply_to_message_id: None,
                forward_of_message_id: None,
                attachments: vec![],
                send_at: Some(now_ms()),
                calendar: None,
            };
            self.send(draft).await?;
            return Ok("mailto".into());
        }
        if let Some(url) = &lu.http {
            (self.inner.opener)(url)?;
            return Ok("browser".into());
        }
        Err(ApiError::invalid("messageId", "This message has no usable unsubscribe link."))
    }
}

/// Exposed for compose (APPEND to the right Drafts/Sent folder).
pub(crate) async fn role_folder(engine: &Engine, account_id: &str, role: FolderRole, default_name: &str) -> Result<(i64, String)> {
    engine.ensure_role_folder(account_id, role, default_name).await
}
