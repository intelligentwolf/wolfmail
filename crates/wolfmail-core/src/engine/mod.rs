//! The engine: one object the Tauri layer holds. Owns the store, the per-account sync runners, the outbox worker
//! and the IntelligentWolf client. Every public `async fn` here is one command in docs/API.md.
// Source: docs/API.md "Commands" and "Events".
mod accounts;
mod compose;
mod iwcmds;
mod messages;
mod runner;

use crate::db::Store;
use crate::error::{ApiError, Result};
use crate::imap::conn::{connect, Conn, Credential};
use crate::model::*;
use crate::oauth;
use crate::secrets::{password_key, SecretStore};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Notify;

pub use compose::DraftSaved;
pub use runner::AccountHandle;

/// Where engine events go (the Tauri layer forwards them to the webview).
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

/// Opens a URL or a file with the system handler.
pub type Opener = Arc<dyn Fn(&str) -> Result<()> + Send + Sync>;

pub struct EngineConfig {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub app_version: String,
}

pub(crate) struct UndoEntry {
    pub expires_at: i64,
    pub moves: Vec<UndoMove>,
}

pub(crate) struct UndoMove {
    pub account_id: String,
    /// Where the messages are now.
    pub current_folder_id: i64,
    /// Where they go back to.
    pub original_folder_id: i64,
    /// UIDs in the current folder when known (COPYUID), else Message-IDs to search for.
    pub uids: Vec<u32>,
    pub message_ids: Vec<String>,
}

pub(crate) struct Inner {
    pub store: Store,
    pub secrets: Arc<dyn SecretStore>,
    pub http: reqwest::Client,
    pub events: Arc<dyn EventSink>,
    pub opener: Opener,
    pub cfg: EngineConfig,
    pub runners: Mutex<HashMap<String, Arc<AccountHandle>>>,
    pub undo: Mutex<HashMap<String, UndoEntry>>,
    pub pair: Mutex<Option<(String, crate::iw::PairStart)>>,
    pub outbox_wake: Notify,
}

#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

impl Engine {
    /// Open (or create) the profile and start syncing every enabled account plus the outbox worker.
    /// Must be called from inside a tokio runtime.
    pub async fn start(cfg: EngineConfig, secrets: Arc<dyn SecretStore>, events: Arc<dyn EventSink>, opener: Opener) -> Result<Engine> {
        crate::tls::install_crypto_provider();
        let store = Store::open(&cfg.data_dir.join("wolfmail.sqlite3"))?;
        let http = reqwest::Client::builder()
            .user_agent(format!("WolfMail/{}", cfg.app_version))
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| ApiError::internal(format!("HTTP client setup failed: {e}")))?;
        let engine = Engine {
            inner: Arc::new(Inner {
                store,
                secrets,
                http,
                events,
                opener,
                cfg,
                runners: Mutex::new(HashMap::new()),
                undo: Mutex::new(HashMap::new()),
                pair: Mutex::new(None),
                outbox_wake: Notify::new(),
            }),
        };
        compose::recover_outbox(&engine).await?;
        for a in engine.accounts_list().await? {
            if a.enabled {
                engine.spawn_runner(&a.id);
            }
        }
        compose::spawn_outbox_worker(engine.clone());
        Ok(engine)
    }

    pub(crate) fn emit(&self, event: &str, payload: Value) {
        self.inner.events.emit(event, payload);
    }

    pub(crate) fn store(&self) -> &Store {
        &self.inner.store
    }

    /// Credential for IMAP (`which` = "imap") or SMTP ("smtp").
    pub(crate) async fn credential(&self, acct: &Account, which: &str) -> Result<Credential> {
        let cfg = if which == "smtp" { &acct.smtp } else { &acct.imap };
        match cfg.auth {
            AuthKind::Password => {
                let pass = match self.inner.secrets.get(&password_key(&acct.id, which))? {
                    Some(p) => p,
                    None => self
                        .inner
                        .secrets
                        .get(&password_key(&acct.id, "imap"))?
                        .ok_or_else(|| ApiError::auth("Enter the password for this account.").with_account(&acct.id))?,
                };
                Ok(Credential::Password { user: cfg.username.clone(), pass })
            }
            AuthKind::OauthGoogle | AuthKind::OauthMicrosoft => {
                let token = oauth::access_token(&self.inner.http, self.inner.secrets.as_ref(), &acct.email)
                    .await
                    .map_err(|e| e.with_account(&acct.id))?;
                Ok(Credential::OAuth { user: cfg.username.clone(), token })
            }
        }
    }

    pub(crate) async fn connect_account(&self, acct: &Account) -> Result<Conn> {
        let cred = self.credential(acct, "imap").await?;
        connect(&acct.imap, &cred).await.map_err(|e| e.with_account(&acct.id))
    }

    pub async fn settings_get(&self) -> Result<Settings> {
        self.store()
            .call(|c| {
                let raw: Option<String> = rusqlite::OptionalExtension::optional(c.query_row("SELECT value FROM settings WHERE key='settings'", [], |r| r.get(0)))?;
                Ok(raw.and_then(|s| serde_json::from_str::<Value>(&s).ok()))
            })
            .await
            .map(|v| merge_settings(Settings::default(), v.unwrap_or(Value::Null)).unwrap_or_default())
    }

    pub async fn settings_set(&self, patch: Value) -> Result<Settings> {
        let cur = self.settings_get().await?;
        let next = merge_settings(cur, patch)?;
        if !(0..=30).contains(&next.undo_send_seconds) {
            return Err(ApiError::invalid("undoSendSeconds", "Undo send can be between 0 and 30 seconds."));
        }
        crate::iw::validate_base(&next.iw_base_url)?;
        let s = serde_json::to_string(&next)?;
        self.store()
            .call(move |c| {
                c.execute("INSERT INTO settings(key, value) VALUES ('settings', ?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [s])?;
                Ok(())
            })
            .await?;
        Ok(next)
    }

    pub async fn sync_now(&self, account_id: Option<String>) -> Result<()> {
        let runners = self.inner.runners.lock();
        for (id, h) in runners.iter() {
            if account_id.as_deref().is_none_or(|a| a == id) {
                h.wake.notify_one();
            }
        }
        Ok(())
    }

    pub(crate) async fn emit_counts(&self) {
        let n = self
            .store()
            .call(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM messages m JOIN folders f ON f.id=m.folder_id JOIN accounts a ON a.id=m.account_id
                     WHERE f.role='inbox' AND m.seen=0 AND a.enabled=1",
                    [],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap_or(0);
        self.emit("counts://changed", serde_json::json!({ "unreadInbox": n }));
    }
}

fn merge_settings(base: Settings, patch: Value) -> Result<Settings> {
    let mut v = serde_json::to_value(&base)?;
    if let (Value::Object(dst), Value::Object(src)) = (&mut v, patch) {
        for (k, val) in src {
            if dst.contains_key(&k) && !val.is_null() {
                dst.insert(k, val);
            }
        }
    }
    serde_json::from_value(v).map_err(|e| ApiError::invalid("settings", format!("A setting has the wrong type: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_merge_ignores_unknown_and_bad_types() {
        // A wrongly typed field rejects the whole patch (shown to the user) rather than half-applying it.
        assert!(merge_settings(Settings::default(), serde_json::json!({ "theme": "dark", "threaded": "yes" })).is_err());
        let s = merge_settings(Settings::default(), serde_json::json!({ "theme": "dark", "bogus": 1, "undoSendSeconds": 5 })).unwrap();
        assert_eq!(s.theme, "dark");
        assert_eq!(s.undo_send_seconds, 5);
    }
}
