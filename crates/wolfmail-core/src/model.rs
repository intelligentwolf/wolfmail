// Source: docs/API.md "Types" — every struct here is the Rust mirror of the TypeScript type of the same name.
// Wire names are camelCase. Ids are strings on the wire; folders/messages are SQLite rowids rendered as strings.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Security {
    Tls,
    Starttls,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthKind {
    Password,
    OauthGoogle,
    OauthMicrosoft,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub username: String,
    pub auth: AuthKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub account_id: String,
    pub name: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default)]
    pub signature_html: String,
    #[serde(default)]
    pub reply_on_top: bool,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountStatus {
    Ok,
    Syncing,
    AuthError,
    Offline,
    Error,
    Disabled,
}

impl AccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AccountStatus::Ok => "ok",
            AccountStatus::Syncing => "syncing",
            AccountStatus::AuthError => "authError",
            AccountStatus::Offline => "offline",
            AccountStatus::Error => "error",
            AccountStatus::Disabled => "disabled",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "syncing" => AccountStatus::Syncing,
            "authError" => AccountStatus::AuthError,
            "offline" => AccountStatus::Offline,
            "error" => AccountStatus::Error,
            "disabled" => AccountStatus::Disabled,
            _ => AccountStatus::Ok,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    Imap,
    Gmail,
    Outlook,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Imap => "imap",
            Provider::Gmail => "gmail",
            Provider::Outlook => "outlook",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "gmail" => Provider::Gmail,
            "outlook" => Provider::Outlook,
            _ => Provider::Imap,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub name: String,
    pub email: String,
    pub color: String,
    pub provider: Provider,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
    pub identities: Vec<Identity>,
    pub body_days: i64,
    pub enabled: bool,
    pub sort_order: i64,
    pub status: AccountStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<i64>,
}

/// Partial<Account> as sent by `account_update`. Only the user-editable fields are honoured.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPatch {
    pub name: Option<String>,
    pub color: Option<String>,
    pub imap: Option<ServerConfig>,
    pub smtp: Option<ServerConfig>,
    pub body_days: Option<i64>,
    pub enabled: Option<bool>,
    pub sort_order: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "camelCase")]
pub enum FolderRole {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
    All,
    Flagged,
    Outbox,
}

impl FolderRole {
    pub fn as_str(self) -> &'static str {
        match self {
            FolderRole::Inbox => "inbox",
            FolderRole::Sent => "sent",
            FolderRole::Drafts => "drafts",
            FolderRole::Trash => "trash",
            FolderRole::Junk => "junk",
            FolderRole::Archive => "archive",
            FolderRole::All => "all",
            FolderRole::Flagged => "flagged",
            FolderRole::Outbox => "outbox",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "inbox" => FolderRole::Inbox,
            "sent" => FolderRole::Sent,
            "drafts" => FolderRole::Drafts,
            "trash" => FolderRole::Trash,
            "junk" => FolderRole::Junk,
            "archive" => FolderRole::Archive,
            "all" => FolderRole::All,
            "flagged" => FolderRole::Flagged,
            "outbox" => FolderRole::Outbox,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub account_id: String,
    pub path: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub role: Option<FolderRole>,
    pub unread: i64,
    pub total: i64,
    pub selectable: bool,
    pub depth: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Address {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub email: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Flags {
    pub seen: bool,
    pub flagged: bool,
    pub answered: bool,
    pub forwarded: bool,
    pub draft: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSummary {
    pub id: String,
    pub account_id: String,
    pub folder_id: String,
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    pub subject: String,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub date: i64,
    pub snippet: String,
    pub flags: Flags,
    pub tags: Vec<String>,
    pub has_attachments: bool,
    pub size: i64,
    pub thread_count: i64,
    pub thread_unread: i64,
    /// Set while the message is snoozed (unix ms it comes back).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    pub inline: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListUnsubscribe {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mailto: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http: Option<String>,
    pub one_click: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthResults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spf: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dkim: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dmarc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFull {
    #[serde(flatten)]
    pub summary: MessageSummary,
    pub bcc: Vec<Address>,
    pub reply_to: Vec<Address>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// True when the message had no HTML part and `html` was generated from the plain text, so the UI can render
    /// it in the app's own theme instead of on a white "paper" like real HTML mail.
    pub plain_text: bool,
    pub remote_content_blocked: bool,
    pub trackers_removed: i64,
    pub attachments: Vec<Attachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_unsubscribe: Option<ListUnsubscribe>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_results: Option<AuthResults>,
    /// Calendar invitation carried by this message (pim/itip.rs), filled in by message_get.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<crate::pim::model::Invite>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnifiedView {
    Inbox,
    Sent,
    Drafts,
    Trash,
    Junk,
    Archive,
    Flagged,
    Unread,
    /// Messages snoozed on this computer (engine/rules.rs).
    Snoozed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewFilter {
    #[serde(default)]
    pub unread: Option<bool>,
    #[serde(default)]
    pub flagged: Option<bool>,
    #[serde(default)]
    pub attachments: Option<bool>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub from_contact: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortOrder {
    DateDesc,
    DateAsc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewQuery {
    #[serde(default)]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub unified: Option<UnifiedView>,
    #[serde(default)]
    pub filter: Option<ViewFilter>,
    #[serde(default)]
    pub search: Option<String>,
    pub threaded: bool,
    pub sort: SortOrder,
    pub offset: i64,
    pub limit: i64,
    /// Rows already on screen in an unread-only list. They stay listed after being read, so opening or selecting
    /// one doesn't make it vanish under the cursor; moved or deleted rows still drop out. Ignored by other views.
    #[serde(default)]
    pub keep_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePage {
    pub total: i64,
    pub items: Vec<MessageSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentFromMessage {
    pub message_id: String,
    pub attachment_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DraftAttachment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_message: Option<AttachmentFromMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DraftInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub identity_id: String,
    #[serde(default)]
    pub to: Vec<Address>,
    #[serde(default)]
    pub cc: Vec<Address>,
    #[serde(default)]
    pub bcc: Vec<Address>,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub html: String,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_of_message_id: Option<String>,
    #[serde(default)]
    pub attachments: Vec<DraftAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_at: Option<i64>,
    /// iMIP part (RFC 6047): an invitation reply/request sent as text/calendar;method=… (calendar invites).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<CalendarPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CalendarPart {
    /// iTIP METHOD: "REPLY", "REQUEST", "CANCEL" …
    pub method: String,
    pub ics: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutboxState {
    Waiting,
    Sending,
    Sent,
    Failed,
}

impl OutboxState {
    pub fn as_str(self) -> &'static str {
        match self {
            OutboxState::Waiting => "waiting",
            OutboxState::Sending => "sending",
            OutboxState::Sent => "sent",
            OutboxState::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "sending" => OutboxState::Sending,
            "sent" => OutboxState::Sent,
            "failed" => OutboxState::Failed,
            _ => OutboxState::Waiting,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxItem {
    pub id: String,
    pub draft: DraftInput,
    pub state: OutboxState,
    pub send_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    #[serde(flatten)]
    pub summary: MessageSummary,
    pub highlight: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: String,
    pub density: String,
    pub list_layout: String,
    pub reading_pane: String,
    pub threaded: bool,
    pub undo_send_seconds: i64,
    pub close_to_tray: bool,
    pub start_in_tray: bool,
    pub notifications: bool,
    pub load_remote_content: String,
    pub mark_read_after_ms: i64,
    pub iw_base_url: String,
    /// The little wolf in the bottom-left corner (Alex from IntelligentWolf), and the name the user calls him.
    pub show_assistant: bool,
    pub assistant_name: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "system".into(),
            density: "comfortable".into(),
            list_layout: "multiline".into(),
            reading_pane: "right".into(),
            threaded: true,
            undo_send_seconds: 10,
            close_to_tray: true,
            start_in_tray: false,
            notifications: true,
            load_remote_content: "never".into(),
            mark_read_after_ms: 1500,
            // Source: docs/IW-INTEGRATION.md "Base URL".
            iw_base_url: "https://intelligentwolf.com/api".into(),
            show_assistant: true,
            // Paul, 28 Oct 2026: "a little wolf … called Mike (the user can change them)".
            assistant_name: "Mike".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub key: String,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagInput {
    #[serde(default)]
    pub key: Option<String>,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportIdentity {
    pub name: String,
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub signature_html: String,
    pub reply_on_top: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportAccount {
    pub name: String,
    pub email: String,
    pub provider: Provider,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
    pub identities: Vec<ImportIdentity>,
    pub folder_count: i64,
    pub local_size_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCandidate {
    pub source: String,
    pub profile_path: String,
    pub accounts: Vec<ImportAccount>,
    pub tags: Vec<Tag>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AutodetectResult {
    pub provider: Provider,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountTestResult {
    pub imap_ok: bool,
    pub smtp_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imap_error: Option<crate::error::ApiError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smtp_error: Option<crate::error::ApiError>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAccount {
    pub name: String,
    pub email: String,
    pub display_name: String,
    pub provider: Provider,
    pub imap: ServerConfig,
    pub smtp: ServerConfig,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
}
