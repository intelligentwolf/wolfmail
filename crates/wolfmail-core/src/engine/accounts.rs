//! Accounts, identities, folders, tags, OAuth, autodetect, import.
// Source: docs/API.md commands accounts_* / account_* / identity_* / folder_* / tags_* / tag_upsert / oauth_authorize
// / import_scan / import_apply.
use super::Engine;
use crate::error::{ApiError, Result};
use crate::imap::conn::{connect, Credential};
use crate::model::*;
use crate::oauth::{self, OAuthProvider};
use crate::secrets::{oauth_key, password_key};
use crate::{autodetect, import, smtp, utf7};
use rusqlite::{params, Connection, OptionalExtension};

/// Account colours offered in rotation — IntelligentWolf's chart palette (theme.css :root --chart-1…5 and the
/// dark-mode chart greys), so accounts read as part of the same design system.
const PALETTE: &[&str] = &["#c41e1e", "#2d1b2d", "#7a6a86", "#b7791f", "#8a5a5a", "#9aa7b4", "#d0a24a", "#b07a7a"];

fn load_identities(c: &Connection, account_id: &str) -> Result<Vec<Identity>> {
    let mut st = c.prepare(
        "SELECT id, account_id, name, email, reply_to, signature_html, reply_on_top, is_default FROM identities WHERE account_id=?1 ORDER BY is_default DESC, email",
    )?;
    let v = st
        .query_map(params![account_id], |r| {
            Ok(Identity {
                id: r.get(0)?,
                account_id: r.get(1)?,
                name: r.get(2)?,
                email: r.get(3)?,
                reply_to: r.get(4)?,
                signature_html: r.get(5)?,
                reply_on_top: r.get(6)?,
                is_default: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub(crate) fn load_accounts(c: &Connection) -> Result<Vec<Account>> {
    let mut st = c.prepare(
        "SELECT id, name, email, color, provider, imap_json, smtp_json, body_days, enabled, sort_order, status, status_message, last_sync_at
         FROM accounts ORDER BY sort_order, name",
    )?;
    let rows = st
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, bool>(8)?,
                r.get::<_, i64>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, Option<String>>(11)?,
                r.get::<_, Option<i64>>(12)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for (id, name, email, color, provider, imap, smtp, body_days, enabled, sort_order, status, msg, last) in rows {
        out.push(Account {
            identities: load_identities(c, &id)?,
            id,
            name,
            email,
            color,
            provider: Provider::parse(&provider),
            imap: serde_json::from_str(&imap)?,
            smtp: serde_json::from_str(&smtp)?,
            body_days,
            enabled,
            sort_order,
            status: if enabled { AccountStatus::parse(&status) } else { AccountStatus::Disabled },
            status_message: msg,
            last_sync_at: last,
        });
    }
    Ok(out)
}

fn validate_server(prefix: &str, s: &ServerConfig) -> Result<()> {
    if s.host.trim().is_empty() || s.host.contains(char::is_whitespace) {
        return Err(ApiError::invalid(&format!("{prefix}.host"), "Enter the server name, like mail.example.com."));
    }
    if s.port == 0 {
        return Err(ApiError::invalid(&format!("{prefix}.port"), "Enter the port number."));
    }
    if s.security == Security::None && !crate::imap::conn::is_localhost(&s.host) {
        return Err(ApiError::invalid(&format!("{prefix}.security"), "Choose SSL/TLS or STARTTLS — unencrypted connections are only allowed to this computer."));
    }
    Ok(())
}

fn folder_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Folder> {
    Ok(Folder {
        id: r.get::<_, i64>(0)?.to_string(),
        account_id: r.get(1)?,
        path: r.get(2)?,
        name: r.get(3)?,
        parent_id: r.get::<_, Option<i64>>(4)?.map(|v| v.to_string()),
        role: r.get::<_, Option<String>>(5)?.as_deref().and_then(FolderRole::parse),
        selectable: r.get(6)?,
        depth: r.get(7)?,
        unread: r.get(8)?,
        total: r.get(9)?,
    })
}

// Counts cover a folder's own rows plus Gmail-API label members (message_labels); outside Trash/Spam a message that
// carries the TRASH/SPAM label is not counted, as Gmail hides it from its other labels (msgstore::NOT_IN_TRASH_LABEL).
// Two indexed sub-counts instead of one OR so SQLite can use messages_folder_date and message_labels_folder.
const FOLDER_SQL: &str = "SELECT f.id, f.account_id, f.path, f.name, f.parent_id, f.role, f.selectable, f.depth,
        (SELECT COUNT(*) FROM messages m WHERE m.folder_id=f.id AND m.seen=0 AND (COALESCE(f.role,'') IN ('trash','junk') OR NOT EXISTS
            (SELECT 1 FROM message_labels xl JOIN folders xf ON xf.id=xl.folder_id WHERE xl.message_rowid=m.id AND xf.role IN ('trash','junk'))))
      + (SELECT COUNT(*) FROM message_labels ml JOIN messages m ON m.id=ml.message_rowid WHERE ml.folder_id=f.id AND m.seen=0
            AND (COALESCE(f.role,'') IN ('trash','junk') OR NOT EXISTS
            (SELECT 1 FROM message_labels xl JOIN folders xf ON xf.id=xl.folder_id WHERE xl.message_rowid=m.id AND xf.role IN ('trash','junk')))),
        (SELECT COUNT(*) FROM messages m WHERE m.folder_id=f.id AND (COALESCE(f.role,'') IN ('trash','junk') OR NOT EXISTS
            (SELECT 1 FROM message_labels xl JOIN folders xf ON xf.id=xl.folder_id WHERE xl.message_rowid=m.id AND xf.role IN ('trash','junk'))))
      + (SELECT COUNT(*) FROM message_labels ml JOIN messages m ON m.id=ml.message_rowid WHERE ml.folder_id=f.id
            AND (COALESCE(f.role,'') IN ('trash','junk') OR NOT EXISTS
            (SELECT 1 FROM message_labels xl JOIN folders xf ON xf.id=xl.folder_id WHERE xl.message_rowid=m.id AND xf.role IN ('trash','junk'))))
    FROM folders f";

pub(crate) fn folder_by_id(c: &Connection, id: i64) -> Result<Folder> {
    c.query_row(&format!("{FOLDER_SQL} WHERE f.id=?1"), params![id], folder_from_row)
        .optional()?
        .ok_or_else(|| ApiError::not_found("The folder"))
}

pub(crate) fn folder_with_role(c: &Connection, account_id: &str, role: FolderRole) -> Result<Option<(i64, String)>> {
    Ok(c.query_row("SELECT id, path FROM folders WHERE account_id=?1 AND role=?2", params![account_id, role.as_str()], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?)
}

pub(crate) fn delimiter_for(c: &Connection, account_id: &str) -> Result<String> {
    Ok(c.query_row(
        "SELECT delimiter FROM folders WHERE account_id=?1 AND delimiter IS NOT NULL AND delimiter<>'' ORDER BY (role='inbox') DESC LIMIT 1",
        params![account_id],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .unwrap_or_else(|| "/".into()))
}

fn parse_id(id: &str, what: &str) -> Result<i64> {
    id.parse().map_err(|_| ApiError::not_found(what))
}

impl Engine {
    pub async fn accounts_list(&self) -> Result<Vec<Account>> {
        self.store().call(|c| load_accounts(c)).await
    }

    pub(crate) async fn account(&self, id: &str) -> Result<Account> {
        let id = id.to_string();
        self.accounts_list().await?.into_iter().find(|a| a.id == id).ok_or_else(|| ApiError::not_found("The account"))
    }

    pub async fn account_autodetect(&self, email: String) -> Result<AutodetectResult> {
        autodetect::autodetect(&self.inner.http, &email).await
    }

    async fn test_credential(&self, cfg: &ServerConfig, password: Option<&str>) -> Result<Credential> {
        match cfg.auth {
            AuthKind::Password => Ok(Credential::Password {
                user: cfg.username.clone(),
                pass: password.map(|s| s.to_string()).ok_or_else(|| ApiError::invalid("password", "Enter the password."))?,
            }),
            _ => Ok(Credential::OAuth {
                user: cfg.username.clone(),
                token: oauth::access_token(&self.inner.http, self.inner.secrets.as_ref(), &cfg.username).await?,
            }),
        }
    }

    pub async fn account_test(&self, imap: ServerConfig, smtp_cfg: ServerConfig, password: Option<String>) -> Result<AccountTestResult> {
        if crate::providers::backend_for_auth(imap.auth) != crate::providers::Backend::Imap {
            // OAuth accounts use the Gmail API / Microsoft Graph for both reading and sending (providers/mod.rs).
            let r = self.api_test(&imap).await;
            return Ok(AccountTestResult { imap_ok: r.is_ok(), smtp_ok: r.is_ok(), imap_error: r.clone().err(), smtp_error: r.err() });
        }
        let imap_res = async {
            validate_server("imap", &imap)?;
            let cred = self.test_credential(&imap, password.as_deref()).await?;
            let conn = connect(&imap, &cred).await?;
            conn.logout().await;
            Ok::<_, ApiError>(())
        }
        .await;
        let smtp_res = async {
            validate_server("smtp", &smtp_cfg)?;
            let secret = match self.test_credential(&smtp_cfg, password.as_deref()).await? {
                Credential::Password { pass, .. } => smtp::SmtpSecret::Password(pass),
                Credential::OAuth { token, .. } => smtp::SmtpSecret::OAuthToken(token),
            };
            smtp::test(&smtp_cfg, &secret).await
        }
        .await;
        Ok(AccountTestResult {
            imap_ok: imap_res.is_ok(),
            smtp_ok: smtp_res.is_ok(),
            imap_error: imap_res.err(),
            smtp_error: smtp_res.err(),
        })
    }

    pub async fn account_add(&self, n: NewAccount) -> Result<Account> {
        let email = n.email.trim().to_string();
        if !email.contains('@') {
            return Err(ApiError::invalid("email", "Enter a full email address, like name@example.com."));
        }
        validate_server("imap", &n.imap)?;
        validate_server("smtp", &n.smtp)?;
        let oauth = n.imap.auth != AuthKind::Password;
        if oauth && self.inner.secrets.get(&oauth_key(&email))?.is_none() {
            return Err(ApiError::auth("Sign in with the provider first."));
        }
        if !oauth && n.password.as_deref().is_none_or(str::is_empty) {
            return Err(ApiError::invalid("password", "Enter the password."));
        }
        let id = uuid::Uuid::new_v4().to_string();
        if let Some(p) = &n.password {
            self.inner.secrets.set(&password_key(&id, "imap"), p)?;
            self.inner.secrets.set(&password_key(&id, "smtp"), p)?;
        }
        let (aid, e2) = (id.clone(), email.clone());
        let display = if n.display_name.trim().is_empty() { email.clone() } else { n.display_name.trim().to_string() };
        let name = if n.name.trim().is_empty() { email.clone() } else { n.name.trim().to_string() };
        let imap = serde_json::to_string(&n.imap)?;
        let smtp_json = serde_json::to_string(&n.smtp)?;
        let color = n.color.clone();
        let provider = n.provider;
        self.store()
            .call(move |c| {
                let exists: Option<String> = c.query_row("SELECT id FROM accounts WHERE lower(email)=lower(?1)", params![e2], |r| r.get(0)).optional()?;
                if exists.is_some() {
                    return Err(ApiError::invalid("email", format!("{e2} is already set up in WolfMail.")));
                }
                let count: i64 = c.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))?;
                let color = color.unwrap_or_else(|| PALETTE[(count as usize) % PALETTE.len()].to_string());
                c.execute(
                    "INSERT INTO accounts(id, name, email, color, provider, imap_json, smtp_json, body_days, enabled, sort_order, status)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,30,1,?8,'syncing')",
                    params![aid, name, e2, color, provider.as_str(), imap, smtp_json, count],
                )?;
                c.execute(
                    "INSERT INTO identities(id, account_id, name, email, signature_html, reply_on_top, is_default) VALUES (?1,?2,?3,?4,'',0,1)",
                    params![uuid::Uuid::new_v4().to_string(), aid, display, e2],
                )?;
                Ok(())
            })
            .await
            .inspect_err(|_| {
                let _ = self.inner.secrets.delete(&password_key(&id, "imap"));
                let _ = self.inner.secrets.delete(&password_key(&id, "smtp"));
            })?;
        self.spawn_runner(&id);
        self.account(&id).await
    }

    pub async fn account_update(&self, id: String, patch: AccountPatch, password: Option<String>) -> Result<Account> {
        if let Some(s) = &patch.imap {
            validate_server("imap", s)?;
        }
        if let Some(s) = &patch.smtp {
            validate_server("smtp", s)?;
        }
        if let Some(d) = patch.body_days {
            if d < -1 {
                return Err(ApiError::invalid("bodyDays", "Choose how many days of messages to keep offline."));
            }
        }
        let cur = self.account(&id).await?;
        if let Some(p) = password.as_deref().filter(|p| !p.is_empty()) {
            self.inner.secrets.set(&password_key(&id, "imap"), p)?;
            self.inner.secrets.set(&password_key(&id, "smtp"), p)?;
        }
        let aid = id.clone();
        let p2 = patch.clone();
        self.store()
            .call(move |c| {
                let tx = c.transaction()?;
                if let Some(v) = &p2.name {
                    tx.execute("UPDATE accounts SET name=?2 WHERE id=?1", params![aid, v])?;
                }
                if let Some(v) = &p2.color {
                    tx.execute("UPDATE accounts SET color=?2 WHERE id=?1", params![aid, v])?;
                }
                if let Some(v) = &p2.imap {
                    tx.execute("UPDATE accounts SET imap_json=?2 WHERE id=?1", params![aid, serde_json::to_string(v)?])?;
                }
                if let Some(v) = &p2.smtp {
                    tx.execute("UPDATE accounts SET smtp_json=?2 WHERE id=?1", params![aid, serde_json::to_string(v)?])?;
                }
                if let Some(v) = p2.body_days {
                    tx.execute("UPDATE accounts SET body_days=?2 WHERE id=?1", params![aid, v])?;
                }
                if let Some(v) = p2.enabled {
                    tx.execute("UPDATE accounts SET enabled=?2 WHERE id=?1", params![aid, v])?;
                }
                if let Some(v) = p2.sort_order {
                    tx.execute("UPDATE accounts SET sort_order=?2 WHERE id=?1", params![aid, v])?;
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
        let reconnect = patch.imap.is_some() || password.is_some() || patch.enabled.is_some() || patch.body_days.is_some();
        if reconnect {
            self.stop_runner(&id);
            let enabled = patch.enabled.unwrap_or(cur.enabled);
            if enabled {
                self.spawn_runner(&id);
            }
        }
        self.account(&id).await
    }

    pub async fn account_remove(&self, id: String) -> Result<()> {
        let acct = self.account(&id).await?;
        self.stop_runner(&id);
        let aid = id.clone();
        let email = acct.email.clone();
        let others_use_email = self
            .store()
            .call(move |c| {
                c.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE account_id=?1)", params![aid])?;
                c.execute("DELETE FROM thread_ids WHERE account_id=?1", params![aid])?;
                c.execute("DELETE FROM accounts WHERE id=?1", params![aid])?;
                let n: i64 = c.query_row("SELECT COUNT(*) FROM accounts WHERE lower(email)=lower(?1)", params![email], |r| r.get(0))?;
                Ok(n > 0)
            })
            .await?;
        self.inner.secrets.delete(&password_key(&id, "imap"))?;
        self.inner.secrets.delete(&password_key(&id, "smtp"))?;
        if !others_use_email {
            self.inner.secrets.delete(&oauth_key(&acct.email))?;
        }
        self.emit("mail://changed", serde_json::json!({ "folderIds": [] }));
        self.emit_counts().await;
        Ok(())
    }

    pub async fn accounts_reorder(&self, ids: Vec<String>) -> Result<()> {
        self.store()
            .call(move |c| {
                let tx = c.transaction()?;
                for (i, id) in ids.iter().enumerate() {
                    tx.execute("UPDATE accounts SET sort_order=?2 WHERE id=?1", params![id, i as i64])?;
                }
                tx.commit()?;
                Ok(())
            })
            .await
    }

    pub async fn identity_upsert(&self, mut ident: Identity) -> Result<Identity> {
        if !ident.email.contains('@') {
            return Err(ApiError::invalid("email", "Enter the address this identity sends from."));
        }
        if ident.account_id.is_empty() {
            return Err(ApiError::invalid("accountId", "Choose the account this identity belongs to."));
        }
        if ident.id.is_empty() {
            ident.id = uuid::Uuid::new_v4().to_string();
        }
        let i2 = ident.clone();
        self.store()
            .call(move |c| {
                let tx = c.transaction()?;
                let n: i64 = tx.query_row("SELECT COUNT(*) FROM accounts WHERE id=?1", params![i2.account_id], |r| r.get(0))?;
                if n == 0 {
                    return Err(ApiError::not_found("The account"));
                }
                let others: i64 = tx.query_row("SELECT COUNT(*) FROM identities WHERE account_id=?1 AND id<>?2", params![i2.account_id, i2.id], |r| r.get(0))?;
                let is_default = i2.is_default || others == 0;
                if is_default {
                    tx.execute("UPDATE identities SET is_default=0 WHERE account_id=?1", params![i2.account_id])?;
                }
                tx.execute(
                    "INSERT INTO identities(id, account_id, name, email, reply_to, signature_html, reply_on_top, is_default) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
                     ON CONFLICT(id) DO UPDATE SET name=excluded.name, email=excluded.email, reply_to=excluded.reply_to,
                        signature_html=excluded.signature_html, reply_on_top=excluded.reply_on_top, is_default=excluded.is_default",
                    params![i2.id, i2.account_id, i2.name, i2.email, i2.reply_to, i2.signature_html, i2.reply_on_top, is_default],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        let id = ident.id.clone();
        self.store()
            .call(move |c| {
                let account_id: String = c.query_row("SELECT account_id FROM identities WHERE id=?1", params![id], |r| r.get(0))?;
                load_identities(c, &account_id)?.into_iter().find(|i| i.id == id).ok_or_else(|| ApiError::not_found("The identity"))
            })
            .await
    }

    pub async fn identity_remove(&self, id: String) -> Result<()> {
        self.store()
            .call(move |c| {
                let (account_id, was_default): (String, bool) = c
                    .query_row("SELECT account_id, is_default FROM identities WHERE id=?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?
                    .ok_or_else(|| ApiError::not_found("The identity"))?;
                let n: i64 = c.query_row("SELECT COUNT(*) FROM identities WHERE account_id=?1", params![account_id], |r| r.get(0))?;
                if n <= 1 {
                    return Err(ApiError::invalid("identity", "An account needs at least one identity to send from."));
                }
                c.execute("DELETE FROM identities WHERE id=?1", params![id])?;
                if was_default {
                    c.execute(
                        "UPDATE identities SET is_default=1 WHERE id=(SELECT id FROM identities WHERE account_id=?1 ORDER BY email LIMIT 1)",
                        params![account_id],
                    )?;
                }
                Ok(())
            })
            .await
    }

    pub async fn oauth_authorize(&self, provider: OAuthProvider, email: String) -> Result<String> {
        let opener = self.inner.opener.clone();
        let open = move |u: &str| opener(u);
        let signed_in = oauth::authorize(&self.inner.http, self.inner.secrets.as_ref(), provider, &email, &open).await?;
        // Any account with this address that was waiting for sign-in can sync now.
        let accounts = self.accounts_list().await?;
        for a in accounts.iter().filter(|a| a.email.eq_ignore_ascii_case(&email) && a.enabled) {
            self.stop_runner(&a.id);
            self.spawn_runner(&a.id);
        }
        Ok(signed_in)
    }

    // ---------------------------------------------------------------- folders

    pub async fn folders_list(&self, account_id: Option<String>) -> Result<Vec<Folder>> {
        self.store()
            .call(move |c| {
                let sql = format!(
                    "{FOLDER_SQL} WHERE (?1 IS NULL OR f.account_id=?1)
                     ORDER BY f.account_id, CASE f.role WHEN 'inbox' THEN 0 WHEN 'drafts' THEN 1 WHEN 'sent' THEN 2 WHEN 'archive' THEN 3
                        WHEN 'junk' THEN 4 WHEN 'trash' THEN 5 WHEN 'all' THEN 6 WHEN 'flagged' THEN 7 ELSE 8 END, f.path COLLATE NOCASE"
                );
                let mut st = c.prepare(&sql)?;
                let v = st.query_map(params![account_id], folder_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await
    }

    pub async fn folder_create(&self, account_id: String, parent_id: Option<String>, name: String) -> Result<Folder> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(ApiError::invalid("name", "Enter a folder name."));
        }
        let pid = parent_id.as_deref().map(|p| parse_id(p, "The parent folder")).transpose()?;
        let acct = self.account(&account_id).await?;
        if crate::providers::backend(&acct) != crate::providers::Backend::Imap {
            if name.contains('/') {
                return Err(ApiError::invalid("name", "A folder name cannot contain “/”."));
            }
            let id = self.api_folder_create(&acct, pid, &name).await?;
            self.emit("mail://changed", serde_json::json!({ "folderIds": [id.to_string()] }));
            return self.store().call(move |c| folder_by_id(c, id)).await;
        }
        let aid = account_id.clone();
        let (delim, parent_path) = self
            .store()
            .call(move |c| {
                let d = delimiter_for(c, &aid)?;
                let pp = match pid {
                    Some(p) => Some(c.query_row("SELECT path FROM folders WHERE id=?1 AND account_id=?2", params![p, aid], |r| r.get::<_, String>(0))?),
                    None => None,
                };
                Ok((d, pp))
            })
            .await?;
        if name.contains(delim.as_str()) {
            return Err(ApiError::invalid("name", format!("A folder name cannot contain “{delim}”.")));
        }
        let path = match parent_path {
            Some(pp) => format!("{pp}{delim}{}", utf7::encode(&name)),
            None => utf7::encode(&name),
        };
        let p2 = path.clone();
        self.with_interactive(&account_id, move |conn| {
            let p = p2.clone();
            Box::pin(async move { conn.create_folder(&p).await })
        })
        .await?;
        let depth = path.matches(delim.as_str()).count() as i64;
        let (aid, p3, n2) = (account_id.clone(), path.clone(), name.clone());
        let id = self
            .store()
            .call(move |c| {
                c.execute(
                    "INSERT INTO folders(account_id, path, name, parent_id, delimiter, selectable, depth) VALUES (?1,?2,?3,?4,?5,1,?6)
                     ON CONFLICT(account_id, path) DO NOTHING",
                    params![aid, p3, n2, pid, delim, depth],
                )?;
                Ok(c.query_row("SELECT id FROM folders WHERE account_id=?1 AND path=?2", params![aid, p3], |r| r.get::<_, i64>(0))?)
            })
            .await?;
        self.emit("mail://changed", serde_json::json!({ "folderIds": [id.to_string()] }));
        self.store().call(move |c| folder_by_id(c, id)).await
    }

    pub async fn folder_rename(&self, id: String, name: String) -> Result<Folder> {
        let fid = parse_id(&id, "The folder")?;
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(ApiError::invalid("name", "Enter a folder name."));
        }
        let f = self.store().call(move |c| folder_by_id(c, fid)).await?;
        if f.role == Some(FolderRole::Inbox) {
            return Err(ApiError::invalid("name", "The Inbox cannot be renamed."));
        }
        let acct = self.account(&f.account_id).await?;
        if crate::providers::backend(&acct) != crate::providers::Backend::Imap {
            if name.contains('/') {
                return Err(ApiError::invalid("name", "A folder name cannot contain “/”."));
            }
            self.api_folder_rename(&acct, &f, &name).await?;
            self.emit("mail://changed", serde_json::json!({ "folderIds": [id] }));
            return self.store().call(move |c| folder_by_id(c, fid)).await;
        }
        let aid = f.account_id.clone();
        let delim = self.store().call(move |c| delimiter_for(c, &aid)).await?;
        if name.contains(delim.as_str()) {
            return Err(ApiError::invalid("name", format!("A folder name cannot contain “{delim}”.")));
        }
        let new_path = match f.path.rsplit_once(delim.as_str()) {
            Some((parent, _)) => format!("{parent}{delim}{}", utf7::encode(&name)),
            None => utf7::encode(&name),
        };
        let (old, new) = (f.path.clone(), new_path.clone());
        self.with_interactive(&f.account_id, move |conn| {
            let (o, n) = (old.clone(), new.clone());
            Box::pin(async move { conn.rename_folder(&o, &n).await })
        })
        .await?;
        let (aid, old, new) = (f.account_id.clone(), f.path.clone(), new_path);
        self.store()
            .call(move |c| {
                let tx = c.transaction()?;
                // Children move with their parent (RFC 3501 §6.3.5).
                tx.execute(
                    "UPDATE folders SET path = ?3 || substr(path, length(?2) + 1) WHERE account_id=?1 AND substr(path, 1, length(?2) + length(?4)) = ?2 || ?4",
                    params![aid, old, new, delim],
                )?;
                tx.execute("UPDATE folders SET path=?3, name=?4 WHERE account_id=?1 AND path=?2", params![aid, old, new, name])?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        self.emit("mail://changed", serde_json::json!({ "folderIds": [id] }));
        self.store().call(move |c| folder_by_id(c, fid)).await
    }

    pub async fn folder_delete(&self, id: String) -> Result<()> {
        let fid = parse_id(&id, "The folder")?;
        let f = self.store().call(move |c| folder_by_id(c, fid)).await?;
        if matches!(f.role, Some(FolderRole::Inbox) | Some(FolderRole::Trash) | Some(FolderRole::Sent) | Some(FolderRole::Drafts)) {
            return Err(ApiError::invalid("folder", format!("{} is a system folder and cannot be deleted.", f.name)));
        }
        let acct = self.account(&f.account_id).await?;
        if crate::providers::backend(&acct) != crate::providers::Backend::Imap {
            self.api_folder_delete(&acct, &f).await?;
            self.emit("mail://changed", serde_json::json!({ "folderIds": [id] }));
            self.emit_counts().await;
            return Ok(());
        }
        let path = f.path.clone();
        self.with_interactive(&f.account_id, move |conn| {
            let p = path.clone();
            Box::pin(async move { conn.delete_folder(&p).await })
        })
        .await?;
        let (aid, path) = (f.account_id.clone(), f.path.clone());
        self.store()
            .call(move |c| {
                let d = delimiter_for(c, &aid)?;
                let ids: Vec<i64> = {
                    let mut st = c.prepare("SELECT id FROM folders WHERE account_id=?1 AND (path=?2 OR substr(path,1,length(?2)+length(?3))=?2||?3)")?;
                    let v = st.query_map(params![aid, path, d], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                    v
                };
                for fid in ids {
                    c.execute("DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE folder_id=?1)", params![fid])?;
                    c.execute("DELETE FROM folders WHERE id=?1", params![fid])?;
                }
                Ok(())
            })
            .await?;
        self.emit("mail://changed", serde_json::json!({ "folderIds": [id] }));
        self.emit_counts().await;
        Ok(())
    }

    pub async fn folder_mark_read(&self, id: String) -> Result<()> {
        let fid = parse_id(&id, "The folder")?;
        let f = self.store().call(move |c| folder_by_id(c, fid)).await?;
        let acct = self.account(&f.account_id).await?;
        if crate::providers::backend(&acct) != crate::providers::Backend::Imap {
            self.api_folder_mark_read(&acct, fid).await?;
            self.emit("mail://changed", serde_json::json!({ "folderIds": [id] }));
            self.emit_counts().await;
            return Ok(());
        }
        let path = f.path.clone();
        self.with_interactive(&f.account_id, move |conn| {
            let p = path.clone();
            Box::pin(async move {
                conn.ensure_selected(&p).await?;
                conn.raw("UID STORE 1:* +FLAGS.SILENT (\\Seen)").await.map(|_| ())
            })
        })
        .await?;
        self.store()
            .call(move |c| {
                c.execute("UPDATE messages SET seen=1 WHERE folder_id=?1", params![fid])?;
                Ok(())
            })
            .await?;
        self.emit("mail://changed", serde_json::json!({ "folderIds": [id] }));
        self.emit_counts().await;
        Ok(())
    }

    // ---------------------------------------------------------------- tags

    pub async fn tags_list(&self) -> Result<Vec<Tag>> {
        self.store()
            .call(|c| {
                let mut st = c.prepare("SELECT key, name, color FROM tags ORDER BY sort_order, name")?;
                let v = st.query_map([], |r| Ok(Tag { key: r.get(0)?, name: r.get(1)?, color: r.get(2)? }))?.collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await
    }

    pub async fn tag_upsert(&self, t: TagInput) -> Result<Tag> {
        let name = t.name.trim().to_string();
        if name.is_empty() {
            return Err(ApiError::invalid("name", "Enter a tag name."));
        }
        if !t.color.starts_with('#') || t.color.len() != 7 {
            return Err(ApiError::invalid("color", "Choose a colour."));
        }
        // IMAP keywords are atoms (RFC 3501 §9 flag-keyword = atom): letters, digits and a few symbols only.
        let key = t.key.clone().filter(|k| !k.is_empty()).unwrap_or_else(|| {
            let slug: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
            format!("wm_{slug}")
        });
        let (k, n, col) = (key.clone(), name.clone(), t.color.clone());
        self.store()
            .call(move |c| {
                let max: i64 = c.query_row("SELECT COALESCE(MAX(sort_order),0) FROM tags", [], |r| r.get(0))?;
                c.execute(
                    "INSERT INTO tags(key, name, color, sort_order) VALUES (?1,?2,?3,?4) ON CONFLICT(key) DO UPDATE SET name=excluded.name, color=excluded.color",
                    params![k, n, col, max + 1],
                )?;
                Ok(())
            })
            .await?;
        Ok(Tag { key, name, color: t.color })
    }

    // ---------------------------------------------------------------- import

    pub async fn import_scan(&self, profile_path: Option<String>) -> Result<Vec<ImportCandidate>> {
        tokio::task::spawn_blocking(move || import::scan(profile_path.as_deref())).await?
    }

    pub async fn import_apply(&self, profile_path: String, emails: Vec<String>) -> Result<Vec<Account>> {
        let pp = profile_path.clone();
        let cand = tokio::task::spawn_blocking(move || import::read_profile(std::path::Path::new(&pp))).await??;
        let wanted: Vec<String> = emails.iter().map(|e| e.to_ascii_lowercase()).collect();
        let picked: Vec<ImportAccount> = cand.accounts.into_iter().filter(|a| wanted.contains(&a.email.to_ascii_lowercase())).collect();
        if picked.is_empty() {
            return Err(ApiError::invalid("emails", "Choose at least one account to import."));
        }
        let tags = cand.tags.clone();
        let created: Vec<String> = self
            .store()
            .call(move |c| {
                let tx = c.transaction()?;
                for t in &tags {
                    tx.execute(
                        "INSERT INTO tags(key, name, color, sort_order) VALUES (?1,?2,?3,(SELECT COALESCE(MAX(sort_order),0)+1 FROM tags))
                         ON CONFLICT(key) DO UPDATE SET name=excluded.name, color=excluded.color",
                        params![t.key, t.name, t.color],
                    )?;
                }
                let mut ids = Vec::new();
                for a in picked {
                    let exists: Option<String> = tx.query_row("SELECT id FROM accounts WHERE lower(email)=lower(?1)", params![a.email], |r| r.get(0)).optional()?;
                    if exists.is_some() {
                        continue;
                    }
                    let id = uuid::Uuid::new_v4().to_string();
                    let count: i64 = tx.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))?;
                    let msg = match a.imap.auth {
                        AuthKind::Password => "Enter the password to start syncing.",
                        _ => "Sign in with the provider to start syncing.",
                    };
                    tx.execute(
                        "INSERT INTO accounts(id, name, email, color, provider, imap_json, smtp_json, body_days, enabled, sort_order, status, status_message)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,30,1,?8,'authError',?9)",
                        params![
                            id,
                            a.name,
                            a.email,
                            PALETTE[(count as usize) % PALETTE.len()],
                            a.provider.as_str(),
                            serde_json::to_string(&a.imap)?,
                            serde_json::to_string(&a.smtp)?,
                            count,
                            msg
                        ],
                    )?;
                    for i in &a.identities {
                        tx.execute(
                            "INSERT INTO identities(id, account_id, name, email, reply_to, signature_html, reply_on_top, is_default) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                            params![uuid::Uuid::new_v4().to_string(), id, i.name, i.email, i.reply_to, i.signature_html, i.reply_on_top, i.is_default],
                        )?;
                    }
                    ids.push(id);
                }
                tx.commit()?;
                Ok(ids)
            })
            .await?;
        // Accounts whose OAuth token is already in the keychain (re-import) can start right away; the rest wait
        // for a password / sign-in, which restarts their runner.
        for id in &created {
            let a = self.account(id).await?;
            let ready = match a.imap.auth {
                AuthKind::Password => self.inner.secrets.get(&password_key(id, "imap"))?.is_some(),
                _ => self.inner.secrets.get(&oauth_key(&a.email))?.is_some(),
            };
            if ready {
                self.spawn_runner(id);
            }
        }
        let all = self.accounts_list().await?;
        Ok(all.into_iter().filter(|a| created.contains(&a.id)).collect())
    }
}
