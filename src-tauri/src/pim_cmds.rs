//! Tauri wrappers for calendar & contacts (docs/API.md "Calendar & contacts"). Kept out of lib.rs so the mail and
//! PIM command lists stay readable and merge independently.
use super::{AppState, R};
use serde_json::{json, Value};
use tauri::State;
use wolfmail_core::oauth::OAuthProvider;
use wolfmail_core::pim::commands::{CalendarPatch, PimAccountPatch};
use wolfmail_core::pim::model::*;

#[tauri::command]
pub async fn pim_accounts_list(s: State<'_, AppState>) -> R<Vec<PimAccount>> {
    s.on(move |e| async move { e.pim_accounts_list().await }).await
}
#[tauri::command]
pub async fn pim_oauth_authorize(s: State<'_, AppState>, provider: OAuthProvider, email: String) -> R<Value> {
    s.on(move |e| async move { e.pim_oauth_authorize(provider, email).await.map(|em| json!({ "email": em })) }).await
}
#[tauri::command]
pub async fn pim_account_add(s: State<'_, AppState>, account: NewPimAccount) -> R<PimAccount> {
    s.on(move |e| async move { e.pim_account_add(account).await }).await
}
#[tauri::command]
pub async fn pim_account_update(s: State<'_, AppState>, id: String, patch: PimAccountPatch, password: Option<String>) -> R<PimAccount> {
    s.on(move |e| async move { e.pim_account_update(id, patch, password).await }).await
}
#[tauri::command]
pub async fn pim_account_remove(s: State<'_, AppState>, id: String) -> R<()> {
    s.on(move |e| async move { e.pim_account_remove(id).await }).await
}
#[tauri::command]
pub async fn pim_sync_now(s: State<'_, AppState>, account_id: Option<String>) -> R<()> {
    s.on(move |e| async move { e.pim_sync_now(account_id).await }).await
}
#[tauri::command]
pub async fn calendars_list(s: State<'_, AppState>) -> R<Vec<Calendar>> {
    s.on(move |e| async move { e.calendars_list().await }).await
}
#[tauri::command]
pub async fn calendar_update(s: State<'_, AppState>, id: String, patch: CalendarPatch) -> R<Calendar> {
    s.on(move |e| async move { e.calendar_update(id, patch).await }).await
}
#[tauri::command]
pub async fn calendar_create(s: State<'_, AppState>, account_id: String, name: String, color: Option<String>) -> R<()> {
    s.on(move |e| async move { e.calendar_create(account_id, name, color).await }).await
}
#[tauri::command]
pub async fn calendar_delete(s: State<'_, AppState>, id: String) -> R<()> {
    s.on(move |e| async move { e.calendar_delete(id).await }).await
}
#[tauri::command]
pub async fn events_range(s: State<'_, AppState>, from: i64, to: i64, calendar_ids: Option<Vec<String>>) -> R<Vec<Occurrence>> {
    s.on(move |e| async move { e.events_range(from, to, calendar_ids).await }).await
}
#[tauri::command]
pub async fn event_get(s: State<'_, AppState>, id: String) -> R<Event> {
    s.on(move |e| async move { e.event_get(id).await }).await
}
#[tauri::command]
pub async fn event_save(s: State<'_, AppState>, event: Event, scope: Option<EditScope>, occurrence_start: Option<i64>) -> R<Event> {
    s.on(move |e| async move { e.event_save(event, scope, occurrence_start).await }).await
}
#[tauri::command]
pub async fn event_delete(s: State<'_, AppState>, id: String, scope: EditScope, occurrence_start: Option<i64>) -> R<()> {
    s.on(move |e| async move { e.event_delete(id, scope, occurrence_start).await }).await
}
#[tauri::command]
pub async fn address_books_list(s: State<'_, AppState>) -> R<Vec<AddressBook>> {
    s.on(move |e| async move { e.address_books_list().await }).await
}
#[tauri::command]
pub async fn contacts_list(s: State<'_, AppState>, book_id: Option<String>, query: Option<String>, limit: Option<i64>) -> R<Vec<Contact>> {
    s.on(move |e| async move { e.contacts_list(book_id, query, limit).await }).await
}
#[tauri::command]
pub async fn contact_get(s: State<'_, AppState>, id: String) -> R<Contact> {
    s.on(move |e| async move { e.contact_get(id).await }).await
}
#[tauri::command]
pub async fn contact_save(s: State<'_, AppState>, contact: Contact) -> R<Contact> {
    s.on(move |e| async move { e.contact_save(contact).await }).await
}
#[tauri::command]
pub async fn contact_delete(s: State<'_, AppState>, id: String) -> R<()> {
    s.on(move |e| async move { e.contact_delete(id).await }).await
}
#[tauri::command]
pub async fn contacts_import_vcf(s: State<'_, AppState>, book_id: String, path: String) -> R<i64> {
    s.on(move |e| async move { e.contacts_import_vcf(book_id, path).await }).await
}
#[tauri::command]
pub async fn contacts_export_vcf(s: State<'_, AppState>, ids: Vec<String>, path: String) -> R<i64> {
    s.on(move |e| async move { e.contacts_export_vcf(ids, path).await }).await
}
#[tauri::command]
pub async fn invite_respond(s: State<'_, AppState>, message_id: String, response: String, calendar_id: Option<String>) -> R<()> {
    s.on(move |e| async move { e.invite_respond(message_id, response, calendar_id).await }).await
}
#[tauri::command]
pub async fn invite_apply(s: State<'_, AppState>, message_id: String) -> R<()> {
    s.on(move |e| async move { e.invite_apply(message_id).await }).await
}
#[tauri::command]
pub async fn pim_import_scan(s: State<'_, AppState>, profile_path: Option<String>) -> R<Vec<PimImportCandidate>> {
    s.on(move |e| async move { e.pim_import_scan(profile_path).await }).await
}
#[tauri::command]
pub async fn pim_import_apply(s: State<'_, AppState>, profile_path: String, items: Vec<PimImportItem>) -> R<Vec<PimAccount>> {
    s.on(move |e| async move { e.pim_import_apply(profile_path, items).await }).await
}
