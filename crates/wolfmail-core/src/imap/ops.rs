//! User actions on the server: flags/keywords, move/copy, delete, append, folder management.
// Source: RFC 3501 §6.4.6 STORE, §6.4.7 COPY, §6.3.3–5 CREATE/DELETE/RENAME, §6.3.11 APPEND;
// RFC 6851 MOVE (fallback COPY + STORE \Deleted + UID EXPUNGE per §3.3); RFC 4315 UID EXPUNGE / COPYUID / APPENDUID.
use super::conn::{map_imap_err, quote, Conn};
use super::proto::{parse_appenduid, parse_copyuid, uid_set};
use crate::error::Result;

impl Conn {
    pub async fn store_flags(&mut self, path: &str, uids: &[u32], add: &[String], remove: &[String]) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        self.ensure_selected(path).await?;
        let set = uid_set(uids);
        if !add.is_empty() {
            self.raw(&format!("UID STORE {set} +FLAGS.SILENT ({})", add.join(" "))).await?;
        }
        if !remove.is_empty() {
            self.raw(&format!("UID STORE {set} -FLAGS.SILENT ({})", remove.join(" "))).await?;
        }
        Ok(())
    }

    /// Move messages; returns destination UIDs in source order when the server reports COPYUID.
    pub async fn move_uids(&mut self, from: &str, uids: &[u32], to: &str) -> Result<Option<Vec<(u32, u32)>>> {
        if uids.is_empty() {
            return Ok(Some(Vec::new()));
        }
        self.ensure_selected(from).await?;
        let set = uid_set(uids);
        let resps = if self.has("MOVE") {
            self.raw(&format!("UID MOVE {set} {}", quote(to))).await?
        } else {
            let r = self.raw(&format!("UID COPY {set} {}", quote(to))).await?;
            self.raw(&format!("UID STORE {set} +FLAGS.SILENT (\\Deleted)")).await?;
            self.expunge_uids(uids).await?;
            r
        };
        Ok(parse_copyuid(&resps).map(|(s, d)| s.into_iter().zip(d).collect()))
    }

    pub async fn copy_uids(&mut self, from: &str, uids: &[u32], to: &str) -> Result<Option<Vec<(u32, u32)>>> {
        if uids.is_empty() {
            return Ok(Some(Vec::new()));
        }
        self.ensure_selected(from).await?;
        let resps = self.raw(&format!("UID COPY {} {}", uid_set(uids), quote(to))).await?;
        Ok(parse_copyuid(&resps).map(|(s, d)| s.into_iter().zip(d).collect()))
    }

    /// Permanently remove messages (mark \Deleted, then UID EXPUNGE when UIDPLUS is there so other \Deleted
    /// messages a different client marked are not swept away; plain EXPUNGE otherwise).
    pub async fn delete_uids(&mut self, path: &str, uids: &[u32]) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        self.ensure_selected(path).await?;
        self.raw(&format!("UID STORE {} +FLAGS.SILENT (\\Deleted)", uid_set(uids))).await?;
        self.expunge_uids(uids).await
    }

    /// Empty a folder (Trash, Junk): every message in it marked \Deleted and expunged. → how many there were. One STORE over
    /// the whole folder (UID 1:*) and one EXPUNGE, never a list of every UID — Junk can hold 80,000+ messages (measured on
    /// Paul's lonewolf@wolfterritories.org, 30 Oct 2026: 84,547), and a command naming them all is too long for some servers.
    /// The whole folder is meant (RFC 3501 §6.4.3: EXPUNGE removes every \Deleted message in the selected mailbox). An empty
    /// folder sends no STORE (some servers refuse a set that matches nothing).
    pub async fn empty_folder(&mut self, path: &str) -> Result<usize> {
        self.select(path, None).await?;
        let n = super::proto::parse_search(&self.raw("UID SEARCH ALL").await?).len();
        if n > 0 {
            self.raw("UID STORE 1:* +FLAGS.SILENT (\\Deleted)").await?;
            self.raw("EXPUNGE").await?;
        }
        Ok(n)
    }

    async fn expunge_uids(&mut self, uids: &[u32]) -> Result<()> {
        if self.has("UIDPLUS") {
            self.raw(&format!("UID EXPUNGE {}", uid_set(uids))).await?;
        } else {
            self.raw("EXPUNGE").await?;
        }
        Ok(())
    }

    /// APPEND a message; returns its UID when the server reports APPENDUID.
    pub async fn append_msg(&mut self, path: &str, flags: &str, raw: &[u8]) -> Result<Option<u32>> {
        // Written out here (not async-imap's `append`) so the tagged OK's [APPENDUID uv uid] is kept.
        // Source: RFC 3501 §6.3.11 + §7.5 — send the literal only after the "+" continuation.
        use async_imap::imap_proto::{Response, Status};
        use tokio::io::AsyncWriteExt;
        let id = self
            .session
            .run_command(format!("APPEND {} ({flags}) {{{}}}", quote(path), raw.len()))
            .await
            .map_err(map_imap_err)?;
        loop {
            let Some(r) = self.session.read_response().await? else {
                return Err(crate::error::ApiError::network("The mail server closed the connection during upload."));
            };
            match r.parsed() {
                Response::Continue { .. } => break,
                Response::Done { tag, outcome, .. } if *tag == id => {
                    return Err(crate::error::ApiError::server(format!(
                        "The mail server refused to store the message: {}",
                        outcome.information.as_deref().unwrap_or("")
                    )));
                }
                _ => {}
            }
        }
        let stream = self.session.as_mut();
        stream.write_all(raw).await?;
        stream.write_all(b"\r\n").await?;
        stream.flush().await?;
        let mut collected = Vec::new();
        loop {
            let Some(rd) = self.session.read_response().await? else {
                return Err(crate::error::ApiError::network("The mail server closed the connection during upload."));
            };
            let r = super::conn::to_owned_resp(&rd.into_owner())?;
            let done = matches!(r.parsed(), Response::Done { tag, .. } if *tag == id);
            let ok = matches!(r.parsed(), Response::Done { status: Status::Ok, .. });
            let info = match r.parsed() {
                Response::Done { outcome, .. } => outcome.information.as_deref().unwrap_or("").to_string(),
                _ => String::new(),
            };
            collected.push(r);
            if done {
                if !ok {
                    return Err(crate::error::ApiError::server(format!("The mail server refused to store the message: {info}")));
                }
                break;
            }
        }
        // APPEND may implicitly change the selected mailbox's state; forget the selection to be safe.
        if self.selected.as_deref() == Some(path) {
            self.selected = None;
        }
        Ok(parse_appenduid(&collected))
    }

    pub async fn find_uid_by_message_id(&mut self, path: &str, message_id: &str) -> Result<Option<u32>> {
        self.select(path, None).await?;
        let resps = self.raw(&format!("UID SEARCH HEADER Message-ID {}", quote(&format!("<{message_id}>")))).await?;
        Ok(super::proto::parse_search(&resps).into_iter().max())
    }

    pub async fn create_folder(&mut self, path: &str) -> Result<()> {
        self.raw(&format!("CREATE {}", quote(path))).await?;
        let _ = self.raw(&format!("SUBSCRIBE {}", quote(path))).await;
        Ok(())
    }

    pub async fn rename_folder(&mut self, from: &str, to: &str) -> Result<()> {
        if self.selected.as_deref() == Some(from) {
            let _ = self.raw("UNSELECT").await;
            self.selected = None;
        }
        self.raw(&format!("RENAME {} {}", quote(from), quote(to))).await?;
        let _ = self.raw(&format!("SUBSCRIBE {}", quote(to))).await;
        Ok(())
    }

    pub async fn delete_folder(&mut self, path: &str) -> Result<()> {
        if self.selected.as_deref() == Some(path) {
            let _ = self.raw("UNSELECT").await;
            self.selected = None;
        }
        let _ = self.raw(&format!("UNSUBSCRIBE {}", quote(path))).await;
        self.raw(&format!("DELETE {}", quote(path))).await?;
        Ok(())
    }
}
