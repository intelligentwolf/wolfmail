//! Drafts (local + server copy in Drafts) and the outbox (undo send, send later, retries, crash-safe).
// Source: docs/API.md draft_save / draft_get / draft_delete / send / outbox_list / outbox_cancel, the
// `outbox://changed` event, and the 2026-10-28 clarification that draft_get/draft_delete accept a Drafts-folder
// message id and that a later save/send REPLACES that server copy.
use super::messages::role_folder;
use super::Engine;
use crate::db::now_ms;
use crate::error::{ApiError, ErrorKind, Result};
use crate::mime;
use crate::model::*;
use crate::msgstore;
use crate::smtp::{self, ResolvedAttachment, SmtpSecret};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::time::Duration;

/// Retry schedule for transient failures (minutes): 1, 2, 5, 10, 30, 60, 120, 240 — then give up.
const RETRY_MIN: &[i64] = &[1, 2, 5, 10, 30, 60, 120, 240];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSaved {
    pub id: String,
    /// False when the local copy is safe but the copy in the server's Drafts folder could not be written yet.
    pub server_saved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_error: Option<String>,
}

fn is_local_draft_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok()
}

impl Engine {
    pub(crate) async fn identity(&self, identity_id: &str) -> Result<(Account, Identity)> {
        for a in self.accounts_list().await? {
            if let Some(i) = a.identities.iter().find(|i| i.id == identity_id).cloned() {
                return Ok((a, i));
            }
        }
        Err(ApiError::invalid("identityId", "Choose who the message is from."))
    }

    async fn resolve_attachments(&self, d: &DraftInput) -> Result<Vec<ResolvedAttachment>> {
        let mut out = Vec::new();
        for a in &d.attachments {
            if let Some(p) = &a.path {
                let path = std::path::Path::new(p);
                let bytes = tokio::fs::read(path)
                    .await
                    .map_err(|e| ApiError::invalid("attachments", format!("Could not read the attachment {p}: {e}")))?;
                out.push(ResolvedAttachment {
                    filename: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "attachment".into()),
                    mime: smtp::mime_for_path(path),
                    bytes,
                });
            } else if let Some(fm) = &a.from_message {
                let rid: i64 = fm.message_id.parse().map_err(|_| ApiError::not_found("The forwarded message"))?;
                let raw = self.raw_bytes(rid).await?;
                let (filename, mime, bytes) =
                    mime::attachment_bytes(&raw, &fm.attachment_id).ok_or_else(|| ApiError::not_found("A forwarded attachment"))?;
                out.push(ResolvedAttachment { filename, mime, bytes });
            }
        }
        Ok(out)
    }

    pub async fn draft_save(&self, mut draft: DraftInput) -> Result<DraftSaved> {
        let (acct, ident) = self.identity(&draft.identity_id).await?;
        // A Drafts-folder message id becomes a local draft that replaces it on the server.
        if let Some(id) = draft.id.clone().filter(|i| !is_local_draft_id(i)) {
            let adopted = self.draft_get(id).await?;
            draft.id = adopted.id;
        }
        let id = draft.id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        draft.id = Some(id.clone());
        let json = serde_json::to_string(&draft)?;
        let (id2, json2) = (id.clone(), json.clone());
        let (old_folder, old_uid, mid): (Option<i64>, Option<i64>, Option<String>) = self
            .store()
            .call(move |c| {
                c.execute(
                    "INSERT INTO drafts(id, json, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET json=excluded.json, updated_at=excluded.updated_at",
                    params![id2, json2, now_ms()],
                )?;
                Ok(c.query_row("SELECT server_folder_id, server_uid, server_message_id FROM drafts WHERE id=?1", params![id2], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?)
            })
            .await?;

        // Server copy: APPEND the new version, then remove the old one.
        let res: Result<()> = async {
            let atts = self.resolve_attachments(&draft).await?;
            let mid = mid.unwrap_or_else(|| smtp::new_message_id(&ident.email));
            let built = smtp::build_message(&draft, &ident, &mid, &atts, now_ms(), false)?;
            let (dfid, dpath) = role_folder(self, &acct.id, FolderRole::Drafts, "Drafts").await?;
            let old_path: Option<String> = match old_folder {
                Some(f) => self.store().call(move |c| Ok(c.query_row("SELECT path FROM folders WHERE id=?1", params![f], |r| r.get(0)).optional()?)).await?,
                None => None,
            };
            let (dp, raw, mid2) = (dpath.clone(), built.copy.clone(), mid.clone());
            let new_uid = self
                .with_interactive(&acct.id, move |conn| {
                    let (dp, raw, mid2) = (dp.clone(), raw.clone(), mid2.clone());
                    Box::pin(async move {
                        let uid = conn.append_msg(&dp, "\\Draft \\Seen", &raw).await?;
                        match uid {
                            Some(u) => Ok(Some(u)),
                            None => conn.find_uid_by_message_id(&dp, &mid2).await,
                        }
                    })
                })
                .await?;
            if let (Some(op), Some(ou)) = (old_path, old_uid) {
                if Some(ou as u32) != new_uid {
                    let ou = ou as u32;
                    let _ = self
                        .with_interactive(&acct.id, move |conn| {
                            let op = op.clone();
                            Box::pin(async move { conn.delete_uids(&op, &[ou]).await })
                        })
                        .await;
                    let of = old_folder.unwrap_or(dfid);
                    let _ = self.store().call(move |c| msgstore::delete_uid(c, of, ou)).await;
                }
            }
            let id3 = id.clone();
            self.store()
                .call(move |c| {
                    c.execute(
                        "UPDATE drafts SET server_folder_id=?2, server_uid=?3, server_message_id=?4 WHERE id=?1",
                        params![id3, dfid, new_uid.map(|u| u as i64), mid],
                    )?;
                    Ok(())
                })
                .await?;
            self.sync_now(Some(acct.id.clone())).await?;
            Ok(())
        }
        .await;
        Ok(DraftSaved { id, server_saved: res.is_ok(), server_error: res.err().map(|e| e.message) })
    }

    pub async fn draft_get(&self, id: String) -> Result<DraftInput> {
        if is_local_draft_id(&id) {
            let id2 = id.clone();
            let json: Option<String> =
                self.store().call(move |c| Ok(c.query_row("SELECT json FROM drafts WHERE id=?1", params![id2], |r| r.get(0)).optional()?)).await?;
            let json = json.ok_or_else(|| ApiError::not_found("The draft"))?;
            let mut d: DraftInput = serde_json::from_str(&json)?;
            d.id = Some(id);
            return Ok(d);
        }
        // A message in a Drafts folder: adopt it as a local draft.
        let rid: i64 = id.parse().map_err(|_| ApiError::not_found("The draft"))?;
        let existing: Option<String> = self
            .store()
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT d.id FROM drafts d JOIN messages m ON m.folder_id=d.server_folder_id AND m.uid=d.server_uid WHERE m.id=?1",
                    params![rid],
                    |r| r.get(0),
                )
                .optional()?)
            })
            .await?;
        if let Some(local) = existing {
            return Box::pin(self.draft_get(local)).await;
        }
        let (folder_id, _, uid, account_id) = self.store().call(move |c| msgstore::locate(c, rid)).await?;
        let raw = self.raw_bytes(rid).await?;
        let h = mime::parse_headers(&raw);
        let body = mime::parse_body(&raw, true);
        let acct = self.account(&account_id).await?;
        let ident = acct
            .identities
            .iter()
            .find(|i| i.email.eq_ignore_ascii_case(&h.from.email))
            .or_else(|| acct.identities.iter().find(|i| i.is_default))
            .or(acct.identities.first())
            .cloned()
            .ok_or_else(|| ApiError::not_found("An identity for this account"))?;
        let local_id = uuid::Uuid::new_v4().to_string();
        // Attachments are copied out now: the server message they live in is replaced on the next save.
        let dir = self.inner.cfg.cache_dir.join("drafts").join(&local_id);
        let mut attachments = Vec::new();
        for a in body.attachments.iter().filter(|a| !a.inline) {
            if let Some((name, _, bytes)) = mime::attachment_bytes(&raw, &a.id) {
                tokio::fs::create_dir_all(&dir).await?;
                let safe: String = name.chars().map(|c| if c.is_alphanumeric() || " ._-()".contains(c) { c } else { '_' }).collect();
                let path = dir.join(if safe.trim_start_matches('.').is_empty() { "attachment".to_string() } else { safe });
                tokio::fs::write(&path, bytes).await?;
                attachments.push(DraftAttachment { path: Some(path.to_string_lossy().to_string()), from_message: None });
            }
        }
        let d = DraftInput {
            id: Some(local_id.clone()),
            identity_id: ident.id.clone(),
            to: h.to.clone(),
            cc: h.cc.clone(),
            bcc: h.bcc.clone(),
            subject: h.subject.clone(),
            html: body.html.clone().unwrap_or_default(),
            text: body.text.clone().unwrap_or_default(),
            in_reply_to: h.in_reply_to.clone(),
            references: h.references.clone(),
            reply_to_message_id: None,
            forward_of_message_id: None,
            attachments,
            send_at: None,
            calendar: None,
        };
        let (json, lid, mid) = (serde_json::to_string(&d)?, local_id.clone(), h.message_id.clone());
        self.store()
            .call(move |c| {
                c.execute(
                    "INSERT INTO drafts(id, json, updated_at, server_folder_id, server_uid, server_message_id) VALUES (?1,?2,?3,?4,?5,?6)",
                    params![lid, json, now_ms(), folder_id, uid as i64, mid],
                )?;
                Ok(())
            })
            .await?;
        Ok(d)
    }

    /// Remove a draft's server copy (if any) and the local row.
    async fn discard_draft(&self, id: &str) -> Result<()> {
        let id2 = id.to_string();
        let row: Option<(Option<i64>, Option<i64>)> = self
            .store()
            .call(move |c| Ok(c.query_row("SELECT server_folder_id, server_uid FROM drafts WHERE id=?1", params![id2], |r| Ok((r.get(0)?, r.get(1)?))).optional()?))
            .await?;
        if let Some((Some(fid), Some(uid))) = row {
            let info: Option<(String, String)> = self
                .store()
                .call(move |c| Ok(c.query_row("SELECT account_id, path FROM folders WHERE id=?1", params![fid], |r| Ok((r.get(0)?, r.get(1)?))).optional()?))
                .await?;
            if let Some((aid, path)) = info {
                let u = uid as u32;
                self.with_interactive(&aid, move |conn| {
                    let path = path.clone();
                    Box::pin(async move { conn.delete_uids(&path, &[u]).await })
                })
                .await?;
                self.store().call(move |c| msgstore::delete_uid(c, fid, u)).await?;
                self.emit("mail://changed", serde_json::json!({ "folderIds": [fid.to_string()] }));
            }
        }
        let (id3, dir) = (id.to_string(), self.inner.cfg.cache_dir.join("drafts").join(id));
        self.store()
            .call(move |c| {
                c.execute("DELETE FROM drafts WHERE id=?1", params![id3])?;
                Ok(())
            })
            .await?;
        let _ = tokio::fs::remove_dir_all(dir).await;
        Ok(())
    }

    pub async fn draft_delete(&self, id: String) -> Result<()> {
        if is_local_draft_id(&id) {
            return self.discard_draft(&id).await;
        }
        let rid: i64 = id.parse().map_err(|_| ApiError::not_found("The draft"))?;
        let (fid, path, uid, aid) = self.store().call(move |c| msgstore::locate(c, rid)).await?;
        self.with_interactive(&aid, move |conn| {
            let path = path.clone();
            Box::pin(async move { conn.delete_uids(&path, &[uid]).await })
        })
        .await?;
        self.store()
            .call(move |c| {
                c.execute("DELETE FROM drafts WHERE server_folder_id=?1 AND server_uid=?2", params![fid, uid as i64])?;
                msgstore::delete_row(c, rid)
            })
            .await?;
        self.emit("mail://changed", serde_json::json!({ "folderIds": [fid.to_string()] }));
        Ok(())
    }

    pub async fn send(&self, mut draft: DraftInput) -> Result<OutboxItem> {
        let (_, ident) = self.identity(&draft.identity_id).await?;
        // Validate now so the user sees a bad address immediately, not after the undo delay.
        smtp::build_message(&draft, &ident, "check@wolfmail.invalid", &[], now_ms(), true)?;
        for a in &draft.attachments {
            if let Some(p) = &a.path {
                if tokio::fs::metadata(p).await.is_err() {
                    return Err(ApiError::invalid("attachments", format!("The attachment {p} no longer exists.")));
                }
            }
        }
        if let Some(id) = draft.id.clone().filter(|i| !is_local_draft_id(i)) {
            draft.id = self.draft_get(id).await?.id;
        }
        let settings = self.settings_get().await?;
        let send_at = draft.send_at.filter(|t| *t > now_ms()).unwrap_or_else(|| now_ms() + settings.undo_send_seconds * 1000);
        let id = uuid::Uuid::new_v4().to_string();
        let item = OutboxItem { id: id.clone(), draft: draft.clone(), state: OutboxState::Waiting, send_at, error: None };
        let json = serde_json::to_string(&draft)?;
        self.store()
            .call(move |c| {
                c.execute(
                    "INSERT INTO outbox(id, draft_json, state, send_at, created_at) VALUES (?1,?2,'waiting',?3,?4)",
                    params![id, json, send_at, now_ms()],
                )?;
                Ok(())
            })
            .await?;
        self.emit("outbox://changed", serde_json::to_value(&item)?);
        self.inner.outbox_wake.notify_one();
        Ok(item)
    }

    pub async fn outbox_list(&self) -> Result<Vec<OutboxItem>> {
        let since = now_ms() - 3_600_000;
        self.store()
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT id, draft_json, state, send_at, error FROM outbox WHERE state<>'sent' OR created_at>?1 ORDER BY send_at",
                )?;
                let v = st
                    .query_map(params![since], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, Option<String>>(4)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v.into_iter()
                    .filter_map(|(id, j, s, at, e)| {
                        Some(OutboxItem { id, draft: serde_json::from_str(&j).ok()?, state: OutboxState::parse(&s), send_at: at, error: e })
                    })
                    .collect())
            })
            .await
    }

    pub async fn outbox_cancel(&self, id: String) -> Result<DraftInput> {
        let id2 = id.clone();
        let (json, state) = self
            .store()
            .call(move |c| {
                let tx = c.transaction()?;
                let row: Option<(String, String)> =
                    tx.query_row("SELECT draft_json, state FROM outbox WHERE id=?1", params![id2], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
                let Some((json, state)) = row else { return Err(ApiError::not_found("The queued message")) };
                if state == "waiting" || state == "failed" {
                    tx.execute("DELETE FROM outbox WHERE id=?1", params![id2])?;
                }
                tx.commit()?;
                Ok((json, state))
            })
            .await?;
        match state.as_str() {
            // No event: OutboxState has no "cancelled" value; the caller already has the draft back.
            "waiting" | "failed" => Ok(serde_json::from_str(&json)?),
            "sending" => Err(ApiError::invalid("id", "Too late — the message is being sent right now.")),
            _ => Err(ApiError::invalid("id", "That message has already been sent.")),
        }
    }
}

pub(crate) async fn recover_outbox(engine: &Engine) -> Result<()> {
    // A crash while "sending": the SMTP server may or may not have accepted it. Retrying risks a duplicate,
    // not retrying risks a lost message; a lost message is worse (and the user sees it in the outbox).
    engine
        .store()
        .call(|c| {
            c.execute("UPDATE outbox SET state='waiting', error='WolfMail closed while sending; retrying.' WHERE state='sending'", [])?;
            Ok(())
        })
        .await
}

pub(crate) fn spawn_outbox_worker(engine: Engine) {
    tokio::spawn(async move {
        loop {
            let now = now_ms();
            let due: Vec<(String, String, i64)> = engine
                .store()
                .call(move |c| {
                    let mut st = c.prepare("SELECT id, draft_json, attempts FROM outbox WHERE state='waiting' AND send_at<=?1 AND next_attempt_at<=?1 ORDER BY send_at")?;
                    let v = st.query_map(params![now], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(v)
                })
                .await
                .unwrap_or_default();
            for (id, json, attempts) in due {
                process(&engine, &id, &json, attempts).await;
            }
            let next: Option<i64> = engine
                .store()
                .call(|c| Ok(c.query_row("SELECT MIN(MAX(send_at, next_attempt_at)) FROM outbox WHERE state='waiting'", [], |r| r.get(0))?))
                .await
                .unwrap_or(None);
            let wait = next.map(|t| (t - now_ms()).clamp(50, 60_000)).unwrap_or(60_000);
            tokio::select! {
                _ = engine.inner.outbox_wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(wait as u64)) => {}
            }
        }
    });
}

async fn set_state(engine: &Engine, id: &str, state: OutboxState, error: Option<String>, next_attempt: Option<i64>) {
    let (i, e) = (id.to_string(), error.clone());
    let row = engine
        .store()
        .call(move |c| {
            c.execute(
                "UPDATE outbox SET state=?2, error=?3, next_attempt_at=COALESCE(?4, next_attempt_at), attempts=attempts+CASE WHEN ?4 IS NULL THEN 0 ELSE 1 END WHERE id=?1",
                params![i, state.as_str(), e, next_attempt],
            )?;
            Ok(c.query_row("SELECT draft_json, send_at FROM outbox WHERE id=?1", params![i], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .optional()?)
        })
        .await;
    if let Ok(Some((json, send_at))) = row {
        if let Ok(draft) = serde_json::from_str::<DraftInput>(&json) {
            let item = OutboxItem { id: id.to_string(), draft, state, send_at, error };
            if let Ok(v) = serde_json::to_value(&item) {
                engine.emit("outbox://changed", v);
            }
        }
    }
}

async fn process(engine: &Engine, id: &str, json: &str, attempts: i64) {
    let Ok(draft) = serde_json::from_str::<DraftInput>(json) else {
        set_state(engine, id, OutboxState::Failed, Some("The queued message is unreadable.".into()), None).await;
        return;
    };
    set_state(engine, id, OutboxState::Sending, None, None).await;
    let r = send_one(engine, &draft).await;
    match r {
        Ok(()) => {
            set_state(engine, id, OutboxState::Sent, None, None).await;
        }
        Err((e, transient)) => {
            let n = attempts as usize;
            if transient && n < RETRY_MIN.len() {
                let next = now_ms() + RETRY_MIN[n] * 60_000;
                set_state(engine, id, OutboxState::Waiting, Some(format!("{} Trying again in {} min.", e.message, RETRY_MIN[n])), Some(next)).await;
            } else {
                set_state(engine, id, OutboxState::Failed, Some(e.message), None).await;
            }
        }
    }
}

async fn send_one(engine: &Engine, draft: &DraftInput) -> std::result::Result<(), (ApiError, bool)> {
    let permanent = |e: ApiError| {
        let transient = matches!(e.kind, ErrorKind::Network);
        (e, transient)
    };
    let (acct, ident) = engine.identity(&draft.identity_id).await.map_err(permanent)?;
    let atts = engine.resolve_attachments(draft).await.map_err(permanent)?;
    let mid = smtp::new_message_id(&ident.email);
    let built = smtp::build_message(draft, &ident, &mid, &atts, now_ms(), true).map_err(permanent)?;
    let secret = match engine.credential(&acct, "smtp").await.map_err(permanent)? {
        crate::imap::conn::Credential::Password { pass, .. } => SmtpSecret::Password(pass),
        crate::imap::conn::Credential::OAuth { token, .. } => SmtpSecret::OAuthToken(token),
    };
    smtp::send(&acct.smtp, &secret, &built).await.map_err(|f| (f.error, f.transient))?;

    // Sent. Everything below is bookkeeping; a failure here must not make the user resend.
    if acct.provider == Provider::Imap {
        // Gmail and Microsoft 365 file submitted mail in Sent themselves; other servers need the APPEND.
        if let Ok((_, sent_path)) = role_folder(engine, &acct.id, FolderRole::Sent, "Sent").await {
            let raw = built.copy.clone();
            let _ = engine
                .with_interactive(&acct.id, move |conn| {
                    let (p, raw) = (sent_path.clone(), raw.clone());
                    Box::pin(async move { conn.append_msg(&p, "\\Seen", &raw).await.map(|_| ()) })
                })
                .await;
        }
    }
    if let Some(m) = &draft.reply_to_message_id {
        let _ = engine.messages_set_flags(vec![m.clone()], vec!["answered".into()], vec![]).await;
    }
    if let Some(m) = &draft.forward_of_message_id {
        let _ = engine.messages_set_flags(vec![m.clone()], vec!["forwarded".into()], vec![]).await;
    }
    let recips: Vec<Address> = draft.to.iter().chain(&draft.cc).chain(&draft.bcc).cloned().collect();
    let _ = engine
        .store()
        .call(move |c| {
            for a in recips {
                msgstore::note_contact(c, &a, now_ms(), true)?;
            }
            Ok(())
        })
        .await;
    if let Some(did) = draft.id.as_deref().filter(|i| is_local_draft_id(i)) {
        let _ = engine.discard_draft(did).await;
    }
    engine.sync_now(Some(acct.id.clone())).await.ok();
    Ok(())
}
