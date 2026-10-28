//! Calendar & contacts ("PIM"). Its own store file, its own background loops, its own commands — kept apart from
//! mail so the two can evolve (and be merged) independently. Entry points used by the engine:
//! `Pim::open` (Engine::start), `start_background` (after the engine exists), `Engine::pim_*` commands (commands.rs),
//! `itip::invite_for_raw` (message_get) and `Engine::pim_suggest_merge` (contacts_suggest).
pub mod commands;
pub mod dav;
mod dav_sync;
mod google;
mod graph;
pub mod ical;
pub mod import;
pub mod itip;
pub mod lines;
pub mod model;
pub mod recur;
pub mod store;
pub mod vcard;
pub mod xml;

use crate::db::Store;
use crate::engine::Engine;
use crate::error::{ApiError, ErrorKind, Result};
use crate::oauth;
use model::*;
use std::path::Path;
use std::time::Duration;
use tokio::sync::Notify;

/// Background sync interval for calendar/contacts accounts (on top of syncs after every local edit).
const SYNC_EVERY: Duration = Duration::from_secs(10 * 60);
const REMINDER_TICK: Duration = Duration::from_secs(30);

pub struct Pim {
    pub store: Store,
    pub wake: Notify,
    /// One sync at a time per account: a background sync and a user-triggered one must not both push the same
    /// dirty items (the second PUT would hit 412 and be treated as a conflict).
    sync_locks: parking_lot::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
}

impl Pim {
    pub fn open(data_dir: &Path) -> Result<Pim> {
        let store = Store::open_with(&data_dir.join("pim.sqlite3"), store::MIGRATIONS)?;
        ensure_local(&store)?;
        Ok(Pim { store, wake: Notify::new(), sync_locks: Default::default() })
    }

    pub fn open_in_memory() -> Result<Pim> {
        let store = Store::open_in_memory_with(store::MIGRATIONS)?;
        ensure_local(&store)?;
        Ok(Pim { store, wake: Notify::new(), sync_locks: Default::default() })
    }

    fn lock_for(&self, account_id: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        // parking_lot guard is dropped before any await (it only clones the Arc out).
        self.sync_locks.lock().entry(account_id.to_string()).or_default().clone()
    }
}

/// Every profile has an "On this computer" account with a Home calendar and a Contacts book, so calendar and
/// contacts work with no server at all.
fn ensure_local(s: &Store) -> Result<()> {
    s.with(|c| {
        let has: i64 = c.query_row("SELECT COUNT(*) FROM pim_accounts WHERE kind='local'", [], |r| r.get(0))?;
        if has == 0 {
            let id = store::new_id();
            store::insert_account(
                c,
                &PimAccount {
                    id: id.clone(),
                    kind: PimKind::Local,
                    name: "On this computer".into(),
                    email: None,
                    url: None,
                    username: None,
                    enabled: true,
                    status: "ok".into(),
                    status_message: None,
                    last_sync_at: None,
                },
            )?;
            store::upsert_calendar(c, &id, "home", "Home", "#c41e1e", false, true)?;
            store::upsert_book(c, &id, "contacts", "Contacts", false)?;
        }
        Ok(())
    })
}

pub fn password_key(account_id: &str) -> String {
    format!("pim-password:{account_id}")
}

/// Shared HTTP plumbing for the REST backends (Google, Graph): bearer auth, JSON, error mapping.
pub(crate) async fn rest(
    http: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    token: &str,
    body: Option<serde_json::Value>,
    extra: &[(&str, &str)],
) -> Result<(u16, serde_json::Value)> {
    let mut rq = http.request(method, url).bearer_auth(token);
    for (k, v) in extra {
        rq = rq.header(*k, *v);
    }
    if let Some(b) = body {
        rq = rq.json(&b);
    }
    let resp = rq.send().await?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    let v: serde_json::Value = if text.trim().is_empty() { serde_json::Value::Null } else { serde_json::from_str(&text).unwrap_or(serde_json::Value::Null) };
    match status {
        200..=299 => Ok((status, v)),
        401 => Err(ApiError::auth("Your calendar/contacts sign-in has expired. Sign in again.")),
        403 => {
            let msg = v.pointer("/error/message").and_then(|m| m.as_str()).unwrap_or("permission denied");
            Err(ApiError::new(ErrorKind::Auth, format!("The provider refused access: {msg}")))
        }
        // Callers that need 404/409/410/412 handle them from the Ok path via `rest_any`.
        _ => {
            let msg = v
                .pointer("/error/message")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("HTTP {status}"));
            Err(ApiError::server(format!("The calendar/contacts provider refused the request: {msg}")).with_status(status))
        }
    }
}

trait WithStatus {
    fn with_status(self, s: u16) -> Self;
}
impl WithStatus for ApiError {
    /// Keep the HTTP status in the `field` slot so callers can branch on 404/410/412 (never shown to users).
    fn with_status(mut self, s: u16) -> Self {
        self.field = Some(format!("http:{s}"));
        self
    }
}

pub(crate) fn http_status(e: &ApiError) -> Option<u16> {
    e.field.as_deref().and_then(|f| f.strip_prefix("http:")).and_then(|s| s.parse().ok())
}

impl Engine {
    pub(crate) fn pim(&self) -> &Pim {
        &self.inner.pim
    }

    pub(crate) fn emit_pim_changed(&self) {
        self.emit("pim://changed", serde_json::json!({}));
    }

    /// Access token for a Google/Microsoft calendar account (separate grant from mail; see oauth::pim_key).
    pub(crate) async fn pim_token(&self, acct: &PimAccount) -> Result<String> {
        let email = acct.email.clone().ok_or_else(|| ApiError::auth("This account has no email address to sign in with."))?;
        oauth::access_token_scoped(&self.inner.http, self.inner.secrets.as_ref(), &oauth::pim_key(&email), &oauth::pim_scopes)
            .await
            .map_err(|e| e.with_account(&acct.id))
    }

    pub(crate) fn pim_password(&self, acct: &PimAccount) -> Result<String> {
        self.inner
            .secrets
            .get(&password_key(&acct.id))?
            .ok_or_else(|| ApiError::auth("Enter the password for this calendar/contacts account.").with_account(&acct.id))
    }

    /// Sync one account now: pull server changes, then push local edits. Status is reported as `pim://status`.
    pub(crate) async fn pim_sync_account(&self, id: &str) -> Result<()> {
        let id_s = id.to_string();
        let acct = self.pim().store.call(move |c| store::account(c, &id_s)).await?;
        if !acct.enabled || acct.kind == PimKind::Local {
            return Ok(());
        }
        let lock = self.pim().lock_for(&acct.id);
        let _guard = lock.lock().await;
        self.pim_set_status(&acct.id, "syncing", None).await;
        let res = match acct.kind {
            PimKind::Dav => dav_sync::sync(self, &acct).await,
            PimKind::Google => google::sync(self, &acct).await,
            PimKind::Microsoft => graph::sync(self, &acct).await,
            PimKind::Local => Ok(()),
        };
        match &res {
            Ok(()) => self.pim_set_status(&acct.id, "ok", None).await,
            Err(e) => {
                let st = match e.kind {
                    ErrorKind::Auth | ErrorKind::OauthConfig => "authError",
                    ErrorKind::Network | ErrorKind::Tls => "offline",
                    _ => "error",
                };
                self.pim_set_status(&acct.id, st, Some(&e.message)).await;
            }
        }
        self.emit_pim_changed();
        res
    }

    async fn pim_set_status(&self, id: &str, status: &str, msg: Option<&str>) {
        let (i, s, m) = (id.to_string(), status.to_string(), msg.map(|x| x.to_string()));
        let _ = self.pim().store.call(move |c| store::set_status(c, &i, &s, m.as_deref())).await;
        self.emit("pim://status", serde_json::json!({ "accountId": id, "status": status, "message": msg }));
    }

    /// Start the sync loop and the reminder loop. Called once from Engine::start.
    pub(crate) fn pim_start_background(&self) {
        let e = self.clone();
        tokio::spawn(async move {
            loop {
                let ids: Vec<String> = e
                    .pim()
                    .store
                    .call(|c| Ok(store::accounts(c)?.into_iter().filter(|a| a.enabled && a.kind != PimKind::Local).map(|a| a.id).collect()))
                    .await
                    .unwrap_or_default();
                for id in ids {
                    if let Err(err) = e.pim_sync_account(&id).await {
                        tracing::info!(account = %id, "calendar/contacts sync: {}", err.message);
                    }
                }
                tokio::select! {
                    _ = e.pim().wake.notified() => {}
                    _ = tokio::time::sleep(SYNC_EVERY) => {}
                }
            }
        });
        let e = self.clone();
        tokio::spawn(async move {
            loop {
                if let Err(err) = commands::fire_due_reminders(&e).await {
                    tracing::warn!("reminders: {}", err.message);
                }
                tokio::time::sleep(REMINDER_TICK).await;
            }
        });
    }
}
