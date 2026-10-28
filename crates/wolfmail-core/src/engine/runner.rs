//! Per-account background work: a sync connection running full passes, an IDLE connection on INBOX that wakes it,
//! and an interactive connection (lazily opened) for the reading pane and user actions — at most three connections
//! per account, so 11 accounts never open dozens.
// Source: RFC 2177 IDLE (re-issue before the 30-minute inactivity limit: we use 25 min); RFC 4549 §4 sync order.
use super::Engine;
use crate::db::now_ms;
use crate::error::{ApiError, ErrorKind, Result};
use crate::imap::conn::Conn;
use crate::imap::sync::{self, AccountCtx};
use crate::model::{Account, AccountStatus, FolderRole};
use crate::msgstore;
use async_imap::extensions::idle::IdleResponse;
use async_imap::imap_proto::{MailboxDatum, Response};
use futures::future::BoxFuture;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

const POLL_EVERY: Duration = Duration::from_secs(5 * 60);
const POLL_NO_IDLE: Duration = Duration::from_secs(2 * 60);
const IDLE_REISSUE: Duration = Duration::from_secs(25 * 60);
const BACKOFF_MIN: Duration = Duration::from_secs(15);
const BACKOFF_MAX: Duration = Duration::from_secs(10 * 60);
/// Gmail API / Graph accounts poll for changes this often while WolfMail runs (no IMAP IDLE; Gmail push needs
/// Cloud Pub/Sub and Graph push needs a public webhook — neither exists for a desktop app). Source: docs/API.md
/// "Engine notes → Providers".
const API_POLL: Duration = Duration::from_secs(30);

pub struct AccountHandle {
    pub wake: Notify,
    pub inbox: Notify,
    pub interactive: tokio::sync::Mutex<Option<Conn>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    idle_started: Mutex<bool>,
}

impl AccountHandle {
    fn new() -> Self {
        AccountHandle {
            wake: Notify::new(),
            inbox: Notify::new(),
            interactive: tokio::sync::Mutex::new(None),
            tasks: Mutex::new(Vec::new()),
            idle_started: Mutex::new(false),
        }
    }
}

fn backoff_next(d: Duration) -> Duration {
    (d * 2).min(BACKOFF_MAX)
}

impl Engine {
    pub(crate) fn spawn_runner(&self, account_id: &str) {
        let handle = Arc::new(AccountHandle::new());
        {
            let mut runners = self.inner.runners.lock();
            if runners.contains_key(account_id) {
                return;
            }
            runners.insert(account_id.to_string(), Arc::clone(&handle));
        }
        let engine = self.clone();
        let id = account_id.to_string();
        let h2 = Arc::clone(&handle);
        let task = tokio::spawn(async move { run(engine, id, h2).await });
        handle.tasks.lock().push(task);
    }

    pub(crate) fn stop_runner(&self, account_id: &str) {
        let h = self.inner.runners.lock().remove(account_id);
        if let Some(h) = h {
            for t in h.tasks.lock().drain(..) {
                t.abort();
            }
        }
    }

    pub(crate) async fn set_status(&self, account_id: &str, status: AccountStatus, message: Option<String>) {
        let (id, msg) = (account_id.to_string(), message.clone());
        let synced = status == AccountStatus::Ok;
        let _ = self
            .store()
            .call(move |c| {
                c.execute(
                    "UPDATE accounts SET status=?2, status_message=?3, last_sync_at=CASE WHEN ?4 THEN ?5 ELSE last_sync_at END WHERE id=?1",
                    rusqlite::params![id, status.as_str(), msg, synced, now_ms()],
                )?;
                Ok(())
            })
            .await;
        self.emit(
            "sync://status",
            serde_json::json!({ "accountId": account_id, "status": status, "message": message }),
        );
    }

    /// Run `f` on the account's interactive connection (opened on first use, reopened once if it dropped).
    pub(crate) async fn with_interactive<T, F>(&self, account_id: &str, f: F) -> Result<T>
    where
        T: Send,
        F: for<'c> Fn(&'c mut Conn) -> BoxFuture<'c, Result<T>> + Send + Sync,
    {
        let acct = self.account(account_id).await?;
        let handle = self.inner.runners.lock().get(account_id).cloned();
        let Some(handle) = handle else {
            // Disabled account: a one-off connection.
            let mut conn = self.connect_account(&acct).await?;
            let r = f(&mut conn).await;
            conn.logout().await;
            return r;
        };
        let mut guard = handle.interactive.lock().await;
        for attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(self.connect_account(&acct).await?);
            }
            let Some(conn) = guard.as_mut() else { continue };
            match f(conn).await {
                Err(e) if e.kind == ErrorKind::Network && attempt == 0 => {
                    *guard = None;
                }
                other => return other,
            }
        }
        Err(ApiError::network("Lost the connection to the mail server.").with_account(account_id))
    }
}

async fn run(engine: Engine, account_id: String, handle: Arc<AccountHandle>) {
    let mut conn: Option<Conn> = None;
    let mut backoff = BACKOFF_MIN;
    let mut inbox_only = false;
    loop {
        let acct = match engine.account(&account_id).await {
            Ok(a) if a.enabled => a,
            _ => return,
        };
        if crate::providers::backend(&acct) != crate::providers::Backend::Imap {
            return run_api(engine, account_id, handle).await;
        }
        if !inbox_only {
            engine.set_status(&account_id, AccountStatus::Syncing, None).await;
        }
        let result = pass(&engine, &acct, &mut conn, inbox_only).await;
        match result {
            Ok((more, has_idle)) => {
                backoff = BACKOFF_MIN;
                engine.set_status(&account_id, AccountStatus::Ok, None).await;
                engine.emit_counts().await;
                if has_idle {
                    let mut started = handle.idle_started.lock();
                    if !*started {
                        *started = true;
                        let (e2, id2, h2) = (engine.clone(), account_id.clone(), Arc::clone(&handle));
                        let t = tokio::spawn(async move { idle_loop(e2, id2, h2).await });
                        handle.tasks.lock().push(t);
                    }
                }
                if more {
                    // Backfill continues in the next pass, but anything the user asked for comes first.
                    inbox_only = false;
                    tokio::select! {
                        _ = handle.wake.notified() => {}
                        _ = handle.inbox.notified() => { inbox_only = true; }
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                    }
                    continue;
                }
                let poll = if has_idle { POLL_EVERY } else { POLL_NO_IDLE };
                tokio::select! {
                    _ = handle.wake.notified() => { inbox_only = false; }
                    _ = handle.inbox.notified() => { inbox_only = true; }
                    _ = tokio::time::sleep(poll) => { inbox_only = false; }
                }
            }
            Err(e) => {
                conn = None;
                inbox_only = false;
                let status = match e.kind {
                    ErrorKind::Auth | ErrorKind::OauthConfig => AccountStatus::AuthError,
                    ErrorKind::Network | ErrorKind::Tls => AccountStatus::Offline,
                    _ => AccountStatus::Error,
                };
                engine.set_status(&account_id, status, Some(e.message.clone())).await;
                if status == AccountStatus::AuthError {
                    // Retrying a refused password locks accounts on many servers — wait for the user.
                    handle.wake.notified().await;
                } else {
                    tokio::select! {
                        _ = handle.wake.notified() => {}
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    backoff = backoff_next(backoff);
                }
            }
        }
    }
}

/// Sync loop for a Gmail-API / Graph account: a pass, then wait for the poll interval or a wake-up.
async fn run_api(engine: Engine, account_id: String, handle: Arc<AccountHandle>) {
    let mut backoff = BACKOFF_MIN;
    let mut first = true;
    loop {
        let acct = match engine.account(&account_id).await {
            Ok(a) if a.enabled => a,
            _ => return,
        };
        if first {
            engine.set_status(&account_id, AccountStatus::Syncing, None).await;
            first = false;
        }
        match engine.api_pass(&acct).await {
            Ok((more, changed, new_inbox)) => {
                backoff = BACKOFF_MIN;
                engine.set_status(&account_id, AccountStatus::Ok, None).await;
                if !changed.is_empty() {
                    engine.emit("mail://changed", serde_json::json!({ "folderIds": changed }));
                    engine.emit_counts().await;
                }
                if !new_inbox.is_empty() {
                    // Filters first; only what is still in the Inbox afterwards is announced.
                    let new_inbox = engine.filters_on_new(&account_id, new_inbox).await;
                    let summaries = engine
                        .store()
                        .call(move |s| {
                            let mut v = Vec::new();
                            for id in new_inbox {
                                if let Ok(m) = msgstore::get_summary(s, id) {
                                    v.push(m);
                                }
                            }
                            Ok(v)
                        })
                        .await
                        .unwrap_or_default();
                    if !summaries.is_empty() {
                        let folder = summaries[0].folder_id.clone();
                        engine.emit("mail://new", serde_json::json!({ "accountId": account_id, "folderId": folder, "messages": summaries }));
                    }
                }
                let wait = if more { Duration::from_millis(500) } else { API_POLL };
                tokio::select! {
                    _ = handle.wake.notified() => {}
                    _ = handle.inbox.notified() => {}
                    _ = tokio::time::sleep(wait) => {}
                }
            }
            Err(e) => {
                let status = match e.kind {
                    ErrorKind::Auth | ErrorKind::OauthConfig => AccountStatus::AuthError,
                    ErrorKind::Network | ErrorKind::Tls => AccountStatus::Offline,
                    _ => AccountStatus::Error,
                };
                engine.set_status(&account_id, status, Some(e.message.clone())).await;
                if status == AccountStatus::AuthError {
                    // Waits for a new sign-in (oauth_authorize restarts the runner) or an explicit sync.
                    handle.wake.notified().await;
                } else {
                    tokio::select! {
                        _ = handle.wake.notified() => {}
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    backoff = backoff_next(backoff);
                }
            }
        }
    }
}

/// One sync pass. Returns (more backfill pending, server supports IDLE).
async fn pass(engine: &Engine, acct: &Account, conn: &mut Option<Conn>, inbox_only: bool) -> Result<(bool, bool)> {
    if conn.is_none() {
        *conn = Some(engine.connect_account(acct).await?);
    }
    let Some(c) = conn.as_mut() else { return Err(ApiError::internal("no connection")) };
    let has_idle = c.has("IDLE");
    let ctx = AccountCtx { account_id: acct.id.clone(), body_days: acct.body_days };
    let folders = if inbox_only {
        let id = acct.id.clone();
        engine
            .store()
            .call(move |s| {
                Ok(super::accounts::folder_with_role(s, &id, FolderRole::Inbox)?
                    .map(|(fid, path)| vec![sync::FolderRef { id: fid, path, role: Some(FolderRole::Inbox) }])
                    .unwrap_or_default())
            })
            .await?
    } else {
        sync::sync_folder_list(c, engine.store(), &acct.id).await?
    };
    let mut more = false;
    for f in folders {
        let out = match sync::sync_folder(c, engine.store(), &ctx, &f).await {
            Ok(o) => o,
            // One broken folder (e.g. deleted mid-sync, or no permission) must not stop the others.
            Err(e) if e.kind == ErrorKind::Server => {
                tracing::warn!(account = %acct.id, folder = %f.path, "folder sync failed: {}", e.message);
                continue;
            }
            Err(e) => return Err(e),
        };
        more |= out.more_backfill;
        if out.changed {
            engine.emit("mail://changed", serde_json::json!({ "folderIds": [f.id.to_string()] }));
        }
        if f.role == Some(FolderRole::Inbox) && !out.new_ids.is_empty() {
            // Filters first; only what is still in the Inbox afterwards is announced.
            let ids = engine.filters_on_new(&acct.id, out.new_ids.clone()).await;
            let summaries = engine
                .store()
                .call(move |s| {
                    let mut v = Vec::new();
                    for id in ids {
                        if let Ok(m) = msgstore::get_summary(s, id) {
                            if !m.flags.seen {
                                v.push(m);
                            }
                        }
                    }
                    Ok(v)
                })
                .await?;
            if !summaries.is_empty() {
                engine.emit(
                    "mail://new",
                    serde_json::json!({ "accountId": acct.id, "folderId": f.id.to_string(), "messages": summaries }),
                );
            }
        }
    }
    Ok((more, has_idle))
}

/// Shortest IDLE refresh WolfMail will use when a server drops idle connections early.
const IDLE_REISSUE_MIN: Duration = Duration::from_secs(45);

async fn idle_loop(engine: Engine, account_id: String, handle: Arc<AccountHandle>) {
    let mut backoff = BACKOFF_MIN;
    // RFC 2177 asks servers to allow ~29 minutes of IDLE, so 25 minutes is the default. Some servers drop idle
    // connections far sooner — StackMail (20i) closes them after exactly 2 minutes (measured 28 Oct 2026) — and
    // every drop meant a full new LOGIN, ~200 logins/hour across Paul's 7 StackMail accounts, after which the
    // server started refusing sign-ins. When IDLE is dropped early, learn the server's limit and re-issue IDLE
    // (DONE + IDLE on the same connection, no new login) at 75% of it from then on.
    let mut reissue = IDLE_REISSUE;
    loop {
        let acct = match engine.account(&account_id).await {
            Ok(a) if a.enabled => a,
            _ => return,
        };
        let inbox_path = {
            let id = account_id.clone();
            engine
                .store()
                .call(move |s| super::accounts::folder_with_role(s, &id, FolderRole::Inbox))
                .await
                .ok()
                .flatten()
                .map(|(_, p)| p)
                .unwrap_or_else(|| "INBOX".to_string())
        };
        let res: Result<()> = async {
            let mut conn = engine.connect_account(&acct).await?;
            conn.select(&inbox_path, None).await?;
            let Conn { mut session, .. } = conn;
            backoff = BACKOFF_MIN;
            loop {
                let mut idle = session.idle();
                idle.init().await.map_err(crate::imap::conn::map_imap_err)?;
                let started = tokio::time::Instant::now();
                let (fut, _stop) = idle.wait_with_timeout(reissue);
                let r = match fut.await {
                    Ok(r) => r,
                    Err(e) => {
                        let lived = started.elapsed();
                        if lived < reissue {
                            let learned = (lived * 3 / 4).max(IDLE_REISSUE_MIN);
                            if learned < reissue {
                                tracing::info!(account = %account_id, "server dropped IDLE after {}s; re-issuing every {}s", lived.as_secs(), learned.as_secs());
                                reissue = learned;
                            }
                        }
                        return Err(crate::imap::conn::map_imap_err(e));
                    }
                };
                drop(_stop);
                session = idle.done().await.map_err(crate::imap::conn::map_imap_err)?;
                match r {
                    IdleResponse::NewData(d) => {
                        let interesting = matches!(
                            d.parsed(),
                            Response::MailboxData(MailboxDatum::Exists(_)) | Response::Expunge(_) | Response::Fetch(..) | Response::Vanished { .. }
                        );
                        if interesting {
                            handle.inbox.notify_one();
                        }
                    }
                    IdleResponse::Timeout => {}
                    IdleResponse::ManualInterrupt => return Ok(()),
                }
            }
        }
        .await;
        if let Err(e) = res {
            tracing::info!(account = %account_id, "IDLE connection ended: {}", e.message);
            if e.kind == ErrorKind::Auth {
                return;
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = backoff_next(backoff);
    }
}
