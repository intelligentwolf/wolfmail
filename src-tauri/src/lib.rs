//! WolfMail desktop shell: starts the engine on its own tokio runtime and exposes every command in docs/API.md.
//! Commands are thin: they hop onto the engine runtime and return the engine's `ApiError` unchanged.
// Source: docs/API.md "Commands" / "Events"; src/lib/api.ts is the UI's binding of the same names (Tauri maps the
// UI's camelCase argument keys to these snake_case parameters).
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_window_state::StateFlags;
use wolfmail_core::engine::{DraftSaved, Engine, EngineConfig, EventSink};
use wolfmail_core::error::{ApiError, ErrorKind};
use wolfmail_core::model::*;
use wolfmail_core::oauth::OAuthProvider;
use wolfmail_core::secrets::KeyringStore;

type R<T> = Result<T, ApiError>;

struct AppState {
    rt: tokio::runtime::Runtime,
    engine: Result<Engine, ApiError>,
    close_to_tray: Arc<AtomicBool>,
}

impl AppState {
    async fn on<T, F, Fut>(&self, f: F) -> R<T>
    where
        F: FnOnce(Engine) -> Fut + Send + 'static,
        Fut: Future<Output = R<T>> + Send + 'static,
        T: Send + 'static,
    {
        let engine = self.engine.clone()?;
        self.rt.spawn(f(engine)).await.map_err(|e| ApiError::internal(format!("The engine task stopped: {e}")))?
    }
}

/// Forwards engine events to the webview and keeps the tray tooltip's unread count current.
struct TauriSink {
    app: AppHandle,
    tray: Arc<Mutex<Option<TrayIcon>>>,
}

impl EventSink for TauriSink {
    fn emit(&self, event: &str, payload: Value) {
        if event == "counts://changed" {
            let n = payload["unreadInbox"].as_i64().unwrap_or(0);
            if let Some(t) = self.tray.lock().as_ref() {
                let tip = if n == 0 { "WolfMail".to_string() } else { format!("WolfMail — {n} unread") };
                let _ = t.set_tooltip(Some(tip));
            }
        }
        let _ = self.app.emit(event, payload);
    }
}

/// The "all inboxes at once" window: a second webview on the same engine. Every engine event is emitted to all
/// windows (`Emitter::emit`), so it stays live without its own sync. Opening it again just focuses it.
fn show_overview(app: &AppHandle) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window("overview") {
        let _ = w.unminimize();
        w.show()?;
        w.set_focus()?;
        return Ok(());
    }
    tauri::WebviewWindowBuilder::new(app, "overview", tauri::WebviewUrl::App("index.html#overview".into()))
        .title("WolfMail — All inboxes")
        .inner_size(1500.0, 900.0)
        .min_inner_size(700.0, 480.0)
        .build()?;
    Ok(())
}

#[tauri::command]
fn overview_window_open(app: AppHandle) -> R<()> {
    show_overview(&app).map_err(|e| ApiError::new(ErrorKind::Internal, format!("Could not open the All inboxes window: {e}")))
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

// ------------------------------------------------------------------------------------------------ accounts

#[tauri::command]
async fn accounts_list(s: State<'_, AppState>) -> R<Vec<Account>> {
    s.on(move |e| async move { e.accounts_list().await }).await
}
#[tauri::command]
async fn account_autodetect(s: State<'_, AppState>, email: String) -> R<AutodetectResult> {
    s.on(move |e| async move { e.account_autodetect(email).await }).await
}
#[tauri::command]
async fn account_test(s: State<'_, AppState>, imap: ServerConfig, smtp: ServerConfig, password: Option<String>) -> R<AccountTestResult> {
    s.on(move |e| async move { e.account_test(imap, smtp, password).await }).await
}
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn account_add(
    s: State<'_, AppState>,
    name: String,
    email: String,
    display_name: String,
    provider: Provider,
    imap: ServerConfig,
    smtp: ServerConfig,
    password: Option<String>,
    color: Option<String>,
) -> R<Account> {
    s.on(move |e| async move { e.account_add(NewAccount { name, email, display_name, provider, imap, smtp, password, color }).await }).await
}
#[tauri::command]
async fn account_update(s: State<'_, AppState>, id: String, patch: AccountPatch, password: Option<String>) -> R<Account> {
    s.on(move |e| async move { e.account_update(id, patch, password).await }).await
}
#[tauri::command]
async fn account_remove(s: State<'_, AppState>, id: String) -> R<()> {
    s.on(move |e| async move { e.account_remove(id).await }).await
}
#[tauri::command]
async fn accounts_reorder(s: State<'_, AppState>, ids: Vec<String>) -> R<()> {
    s.on(move |e| async move { e.accounts_reorder(ids).await }).await
}
#[tauri::command]
async fn identity_upsert(s: State<'_, AppState>, identity: Identity) -> R<Identity> {
    s.on(move |e| async move { e.identity_upsert(identity).await }).await
}
#[tauri::command]
async fn identity_remove(s: State<'_, AppState>, id: String) -> R<()> {
    s.on(move |e| async move { e.identity_remove(id).await }).await
}
#[tauri::command]
async fn oauth_authorize(s: State<'_, AppState>, provider: OAuthProvider, email: String) -> R<Value> {
    s.on(move |e| async move { e.oauth_authorize(provider, email).await.map(|em| json!({ "email": em })) }).await
}

// ------------------------------------------------------------------------------------------------ folders

#[tauri::command]
async fn folders_list(s: State<'_, AppState>, account_id: Option<String>) -> R<Vec<Folder>> {
    s.on(move |e| async move { e.folders_list(account_id).await }).await
}
#[tauri::command]
async fn folder_create(s: State<'_, AppState>, account_id: String, parent_id: Option<String>, name: String) -> R<Folder> {
    s.on(move |e| async move { e.folder_create(account_id, parent_id, name).await }).await
}
#[tauri::command]
async fn folder_rename(s: State<'_, AppState>, id: String, name: String) -> R<Folder> {
    s.on(move |e| async move { e.folder_rename(id, name).await }).await
}
#[tauri::command]
async fn folder_delete(s: State<'_, AppState>, id: String) -> R<()> {
    s.on(move |e| async move { e.folder_delete(id).await }).await
}
#[tauri::command]
async fn folder_mark_read(s: State<'_, AppState>, id: String) -> R<()> {
    s.on(move |e| async move { e.folder_mark_read(id).await }).await
}

// ------------------------------------------------------------------------------------------------ messages

#[tauri::command]
async fn messages_list(s: State<'_, AppState>, query: ViewQuery) -> R<MessagePage> {
    s.on(move |e| async move { e.messages_list(query).await }).await
}
#[tauri::command]
async fn thread_get(s: State<'_, AppState>, thread_id: String, account_id: String) -> R<Vec<MessageSummary>> {
    s.on(move |e| async move { e.thread_get(thread_id, account_id).await }).await
}
#[tauri::command]
async fn message_get(s: State<'_, AppState>, id: String) -> R<MessageFull> {
    s.on(move |e| async move { e.message_get(id).await }).await
}
#[tauri::command]
async fn message_load_remote(s: State<'_, AppState>, id: String) -> R<MessageFull> {
    s.on(move |e| async move { e.message_load_remote(id).await }).await
}
#[tauri::command]
async fn message_raw(s: State<'_, AppState>, id: String) -> R<String> {
    s.on(move |e| async move { e.message_raw(id).await }).await
}
#[tauri::command]
async fn messages_set_flags(s: State<'_, AppState>, ids: Vec<String>, add: Vec<String>, remove: Vec<String>) -> R<()> {
    s.on(move |e| async move { e.messages_set_flags(ids, add, remove).await }).await
}
#[tauri::command]
async fn messages_set_tags(s: State<'_, AppState>, ids: Vec<String>, add: Vec<String>, remove: Vec<String>) -> R<()> {
    s.on(move |e| async move { e.messages_set_tags(ids, add, remove).await }).await
}
#[tauri::command]
async fn messages_move(s: State<'_, AppState>, ids: Vec<String>, folder_id: String) -> R<()> {
    s.on(move |e| async move { e.messages_move(ids, folder_id).await }).await
}
#[tauri::command]
async fn messages_copy(s: State<'_, AppState>, ids: Vec<String>, folder_id: String) -> R<()> {
    s.on(move |e| async move { e.messages_copy(ids, folder_id).await }).await
}
#[tauri::command]
async fn messages_archive(s: State<'_, AppState>, ids: Vec<String>) -> R<()> {
    s.on(move |e| async move { e.messages_archive(ids).await }).await
}
#[tauri::command]
async fn messages_trash(s: State<'_, AppState>, ids: Vec<String>) -> R<()> {
    s.on(move |e| async move { e.messages_trash(ids).await }).await
}
#[tauri::command]
async fn messages_junk(s: State<'_, AppState>, ids: Vec<String>, junk: bool) -> R<()> {
    s.on(move |e| async move { e.messages_junk(ids, junk).await }).await
}
#[tauri::command]
async fn messages_undo(s: State<'_, AppState>, token: String) -> R<()> {
    s.on(move |e| async move { e.messages_undo(token).await }).await
}
#[tauri::command]
async fn attachment_save(s: State<'_, AppState>, message_id: String, attachment_id: String, path: String) -> R<()> {
    s.on(move |e| async move { e.attachment_save(message_id, attachment_id, path).await }).await
}
#[tauri::command]
async fn attachment_open(s: State<'_, AppState>, message_id: String, attachment_id: String) -> R<()> {
    s.on(move |e| async move { e.attachment_open(message_id, attachment_id).await }).await
}

// ------------------------------------------------------------------------------------------------ compose

#[tauri::command]
async fn draft_save(s: State<'_, AppState>, draft: DraftInput) -> R<DraftSaved> {
    s.on(move |e| async move { e.draft_save(draft).await }).await
}
#[tauri::command]
async fn draft_get(s: State<'_, AppState>, id: String) -> R<DraftInput> {
    s.on(move |e| async move { e.draft_get(id).await }).await
}
#[tauri::command]
async fn draft_delete(s: State<'_, AppState>, id: String) -> R<()> {
    s.on(move |e| async move { e.draft_delete(id).await }).await
}
#[tauri::command]
async fn send(s: State<'_, AppState>, draft: DraftInput) -> R<OutboxItem> {
    s.on(move |e| async move { e.send(draft).await }).await
}
#[tauri::command]
async fn outbox_list(s: State<'_, AppState>) -> R<Vec<OutboxItem>> {
    s.on(move |e| async move { e.outbox_list().await }).await
}
#[tauri::command]
async fn outbox_cancel(s: State<'_, AppState>, id: String) -> R<DraftInput> {
    s.on(move |e| async move { e.outbox_cancel(id).await }).await
}

// ------------------------------------------------------------------------------------------------ misc

#[tauri::command]
async fn search(s: State<'_, AppState>, query: String, account_id: Option<String>, limit: Option<i64>) -> R<Vec<SearchHit>> {
    s.on(move |e| async move { e.search(query, account_id, limit.unwrap_or(50)).await }).await
}
#[tauri::command]
async fn contacts_suggest(s: State<'_, AppState>, prefix: String, limit: Option<i64>) -> R<Vec<Address>> {
    s.on(move |e| async move { e.contacts_suggest(prefix, limit.unwrap_or(8)).await }).await
}
#[tauri::command]
async fn sync_now(s: State<'_, AppState>, account_id: Option<String>) -> R<()> {
    s.on(move |e| async move { e.sync_now(account_id).await }).await
}
#[tauri::command]
async fn unsubscribe(s: State<'_, AppState>, message_id: String) -> R<Value> {
    s.on(move |e| async move { e.unsubscribe(message_id).await.map(|m| json!({ "method": m })) }).await
}
#[tauri::command]
async fn tags_list(s: State<'_, AppState>) -> R<Vec<Tag>> {
    s.on(move |e| async move { e.tags_list().await }).await
}
#[tauri::command]
async fn tag_upsert(s: State<'_, AppState>, tag: TagInput) -> R<Tag> {
    s.on(move |e| async move { e.tag_upsert(tag).await }).await
}
#[tauri::command]
async fn settings_get(s: State<'_, AppState>) -> R<Settings> {
    s.on(move |e| async move { e.settings_get().await }).await
}
#[tauri::command]
async fn settings_set(s: State<'_, AppState>, patch: Value) -> R<Settings> {
    let out = s.on(move |e| async move { e.settings_set(patch).await }).await?;
    s.close_to_tray.store(out.close_to_tray, Ordering::Relaxed);
    Ok(out)
}
#[tauri::command]
async fn import_scan(s: State<'_, AppState>, profile_path: Option<String>) -> R<Vec<ImportCandidate>> {
    s.on(move |e| async move { e.import_scan(profile_path).await }).await
}
#[tauri::command]
async fn import_apply(s: State<'_, AppState>, profile_path: String, emails: Vec<String>) -> R<Vec<Account>> {
    s.on(move |e| async move { e.import_apply(profile_path, emails).await }).await
}

// ------------------------------------------------------------------------------------------------ IntelligentWolf

#[tauri::command]
async fn iw_status(s: State<'_, AppState>) -> R<Value> {
    s.on(move |e| async move { serde_json::to_value(e.iw_status().await?).map_err(ApiError::from) }).await
}
#[tauri::command]
async fn iw_pair_start(s: State<'_, AppState>, base_url: Option<String>) -> R<Value> {
    s.on(move |e| async move { serde_json::to_value(e.iw_pair_start(base_url).await?).map_err(ApiError::from) }).await
}
#[tauri::command]
async fn iw_pair_poll(s: State<'_, AppState>) -> R<Value> {
    s.on(move |e| async move { e.iw_pair_poll().await.map(|st| json!({ "state": st })) }).await
}
#[tauri::command]
async fn iw_disconnect(s: State<'_, AppState>) -> R<()> {
    s.on(move |e| async move { e.iw_disconnect().await }).await
}
#[tauri::command]
async fn iw_person(s: State<'_, AppState>, email: String) -> R<Value> {
    s.on(move |e| async move { e.iw_person(email).await }).await
}
#[tauri::command]
async fn iw_ai(s: State<'_, AppState>, action: String, message_id: String, instructions: Option<String>) -> R<Value> {
    s.on(move |e| async move { e.iw_ai(action, message_id, instructions).await }).await
}
#[tauri::command]
async fn iw_log_email(s: State<'_, AppState>, message_id: String, lead_id: Option<String>) -> R<Value> {
    s.on(move |e| async move { e.iw_log_email(message_id, lead_id).await }).await
}
#[tauri::command]
async fn iw_create_task(s: State<'_, AppState>, message_id: String, title: String, due: Option<Value>) -> R<Value> {
    s.on(move |e| async move { e.iw_create_task(message_id, title, due).await }).await
}
#[tauri::command]
async fn iw_booking_link(s: State<'_, AppState>) -> R<Value> {
    s.on(move |e| async move { e.iw_booking_link().await }).await
}
#[tauri::command]
async fn iw_business_mailboxes(s: State<'_, AppState>) -> R<Value> {
    s.on(move |e| async move { e.iw_business_mailboxes().await }).await
}

// ------------------------------------------------------------------------------------------------ app

fn build_tray(app: &AppHandle) -> tauri::Result<TrayIcon> {
    let show = MenuItem::with_id(app, "show", "Show WolfMail", true, None::<&str>)?;
    let overview = MenuItem::with_id(app, "overview", "All inboxes window", true, None::<&str>)?;
    let check = MenuItem::with_id(app, "check", "Check for new mail", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit WolfMail", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &overview, &check, &quit])?;
    let mut b = TrayIconBuilder::with_id("wolfmail").tooltip("WolfMail").menu(&menu).show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        b = b.icon(icon.clone());
    }
    b.on_menu_event(|app, ev| match ev.id().as_ref() {
        "show" => show_main(app),
        "overview" => {
            if let Err(e) = show_overview(app) {
                tracing::warn!("overview window: {e}");
            }
        }
        "check" => {
            let st = app.state::<AppState>();
            if let Ok(e) = st.engine.clone() {
                st.rt.spawn(async move {
                    let _ = e.sync_now(None).await;
                });
            }
        }
        "quit" => app.exit(0),
        _ => {}
    })
    .on_tray_icon_event(|tray, ev| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
            let app = tray.app_handle();
            match app.get_webview_window("main") {
                Some(w) if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) => {
                    let _ = w.hide();
                }
                _ => show_main(app),
            }
        }
    })
    .build(app)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "wolfmail_core=info,wolfmail_app=info".into()))
        .try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_window_state::Builder::new().with_state_flags(StateFlags::all() & !StateFlags::VISIBLE).build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let tray_slot: Arc<Mutex<Option<TrayIcon>>> = Arc::new(Mutex::new(None));
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("wolfmail-engine").build()?;
            let paths = app.path();
            let cfg = EngineConfig {
                data_dir: paths.app_data_dir()?,
                cache_dir: paths.app_cache_dir()?,
                app_version: app.package_info().version.to_string(),
            };
            let opener_handle = handle.clone();
            let opener: wolfmail_core::engine::Opener = Arc::new(move |target: &str| {
                let r = if target.starts_with("http://") || target.starts_with("https://") || target.starts_with("mailto:") {
                    opener_handle.opener().open_url(target, None::<&str>)
                } else {
                    opener_handle.opener().open_path(target, None::<&str>)
                };
                r.map_err(|e| ApiError::new(ErrorKind::Internal, format!("Could not open {target}: {e}")))
            });
            let sink = Arc::new(TauriSink { app: handle.clone(), tray: Arc::clone(&tray_slot) });
            let engine = rt.block_on(Engine::start(cfg, Arc::new(KeyringStore), sink, opener));
            let settings = match &engine {
                Ok(e) => rt.block_on(e.settings_get()).unwrap_or_default(),
                Err(err) => {
                    tracing::error!("engine failed to start: {}", err.message);
                    Settings::default()
                }
            };
            let close_to_tray = Arc::new(AtomicBool::new(settings.close_to_tray));
            app.manage(AppState { rt, engine, close_to_tray: Arc::clone(&close_to_tray) });

            match build_tray(&handle) {
                Ok(t) => *tray_slot.lock() = Some(t),
                // Some Linux desktops have no tray; WolfMail then simply quits on close.
                Err(e) => {
                    tracing::warn!("no system tray: {e}");
                    close_to_tray.store(false, Ordering::Relaxed);
                }
            }
            if let Some(w) = app.get_webview_window("main") {
                let ctt = Arc::clone(&close_to_tray);
                let w2 = w.clone();
                w.on_window_event(move |ev| {
                    if let WindowEvent::CloseRequested { api, .. } = ev {
                        if ctt.load(Ordering::Relaxed) {
                            api.prevent_close();
                            let _ = w2.hide();
                        }
                    }
                });
                if !(settings.start_in_tray && tray_slot.lock().is_some()) {
                    let _ = w.show();
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            overview_window_open,
            accounts_list,
            account_autodetect,
            account_test,
            account_add,
            account_update,
            account_remove,
            accounts_reorder,
            identity_upsert,
            identity_remove,
            oauth_authorize,
            folders_list,
            folder_create,
            folder_rename,
            folder_delete,
            folder_mark_read,
            messages_list,
            thread_get,
            message_get,
            message_load_remote,
            message_raw,
            messages_set_flags,
            messages_set_tags,
            messages_move,
            messages_copy,
            messages_archive,
            messages_trash,
            messages_junk,
            messages_undo,
            attachment_save,
            attachment_open,
            draft_save,
            draft_get,
            draft_delete,
            send,
            outbox_list,
            outbox_cancel,
            search,
            contacts_suggest,
            sync_now,
            unsubscribe,
            tags_list,
            tag_upsert,
            settings_get,
            settings_set,
            import_scan,
            import_apply,
            iw_status,
            iw_pair_start,
            iw_pair_poll,
            iw_disconnect,
            iw_person,
            iw_ai,
            iw_log_email,
            iw_create_task,
            iw_booking_link,
            iw_business_mailboxes,
        ])
        .run(tauri::generate_context!())
        .expect("error while running WolfMail");
}
