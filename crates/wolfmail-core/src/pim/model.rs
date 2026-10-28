//! Calendar & contacts wire types. Mirror: src/lib/pimTypes.ts. Contract: docs/API.md "Calendar & contacts".
use serde::{Deserialize, Serialize};

/// Where calendars/address books come from.
/// - `local`: on this computer only.
/// - `dav`: CalDAV and/or CardDAV server (Nextcloud, Fastmail, iCloud app password, Radicale, …), password auth.
/// - `google`: Google Calendar API v3 + People API (contacts read-only), OAuth.
/// - `microsoft`: Microsoft Graph calendars + contacts, OAuth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PimKind {
    Local,
    Dav,
    Google,
    Microsoft,
}

impl PimKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PimKind::Local => "local",
            PimKind::Dav => "dav",
            PimKind::Google => "google",
            PimKind::Microsoft => "microsoft",
        }
    }
    pub fn parse(s: &str) -> PimKind {
        match s {
            "dav" => PimKind::Dav,
            "google" => PimKind::Google,
            "microsoft" => PimKind::Microsoft,
            _ => PimKind::Local,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PimAccount {
    pub id: String,
    pub kind: PimKind,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub enabled: bool,
    /// "ok" | "syncing" | "authError" | "offline" | "error"
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewPimAccount {
    pub kind: PimKind,
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Calendar {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub color: String,
    pub visible: bool,
    pub read_only: bool,
    pub is_default: bool,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Attendee {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// iCalendar PARTSTAT: "NEEDS-ACTION" | "ACCEPTED" | "DECLINED" | "TENTATIVE" | "DELEGATED"
    #[serde(default = "needs_action")]
    pub partstat: String,
    #[serde(default)]
    pub optional: bool,
}

fn needs_action() -> String {
    "NEEDS-ACTION".into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Organizer {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A stored event: either a series master / single event, or an override of one occurrence
/// (`recurrence_id` set; same `uid` as its master).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    #[serde(default)]
    pub id: String,
    pub calendar_id: String,
    #[serde(default)]
    pub uid: String,
    pub title: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub description: String,
    /// Unix ms. All-day events: midnight UTC of the first day; `end` is exclusive (midnight after the last day).
    pub start: i64,
    pub end: i64,
    #[serde(default)]
    pub all_day: bool,
    /// IANA zone the times were written in ("" = floating / UTC). Display is always in the viewer's zone.
    #[serde(default)]
    pub timezone: String,
    /// RFC 5545 RRULE value without the "RRULE:" prefix, e.g. "FREQ=WEEKLY;BYDAY=MO,WE".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rrule: Option<String>,
    #[serde(default)]
    pub exdates: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<i64>,
    /// "CONFIRMED" | "TENTATIVE" | "CANCELLED"
    #[serde(default = "confirmed")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organizer: Option<Organizer>,
    #[serde(default)]
    pub attendees: Vec<Attendee>,
    /// Minutes before start for each display reminder.
    #[serde(default)]
    pub reminders: Vec<i64>,
    #[serde(default)]
    pub sequence: i64,
    #[serde(default)]
    pub read_only: bool,
}

fn confirmed() -> String {
    "CONFIRMED".into()
}

/// One concrete occurrence in a time range — what the calendar views draw.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Occurrence {
    pub event_id: String,
    pub calendar_id: String,
    pub title: String,
    pub location: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub color: String,
    pub recurring: bool,
    /// Original start of this occurrence (the RECURRENCE-ID it would have); equals `start` unless moved.
    pub occurrence_start: i64,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub my_partstat: Option<String>,
    pub read_only: bool,
    pub has_attendees: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EditScope {
    /// Only the occurrence starting at `occurrence_start`.
    This,
    /// The whole series.
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddressBook {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Labeled {
    /// "home" | "work" | "mobile" | "other" | free text from the source
    #[serde(default)]
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    #[serde(default)]
    pub id: String,
    pub book_id: String,
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub full_name: String,
    #[serde(default)]
    pub given_name: String,
    #[serde(default)]
    pub family_name: String,
    #[serde(default)]
    pub org: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub emails: Vec<Labeled>,
    #[serde(default)]
    pub phones: Vec<Labeled>,
    #[serde(default)]
    pub addresses: Vec<Labeled>,
    #[serde(default)]
    pub notes: String,
    /// "YYYY-MM-DD", or "--MM-DD" when the year is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub birthday: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    /// data: URL (small, inline) when the source provides a photo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<String>,
    #[serde(default)]
    pub read_only: bool,
}

/// What the mail reader shows for a message carrying a text/calendar part (iMIP, RFC 6047).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    /// "REQUEST" | "CANCEL" | "REPLY" | "PUBLISH" | "COUNTER" …
    pub method: String,
    pub uid: String,
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub all_day: bool,
    pub location: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organizer: Option<Organizer>,
    pub attendees: Vec<Attendee>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rrule: Option<String>,
    pub sequence: i64,
    /// The attendee entry that matches one of the user's identities, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub my_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub my_partstat: Option<String>,
    /// Set when this event is already in one of the user's calendars.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_event_id: Option<String>,
    /// Other events overlapping this invite.
    pub conflicts: Vec<Occurrence>,
    /// For a REPLY: who answered and how.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_from: Option<Attendee>,
}

/// What the Betterbird/Thunderbird profile holds for calendars & address books.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PimImportItem {
    /// "caldav" | "carddav" | "local-calendar" | "google-calendar" | "google-contacts"
    pub kind: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub enabled: bool,
    /// Events found in the profile's local calendar store (local-calendar only).
    pub local_events: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PimImportCandidate {
    pub profile_path: String,
    pub items: Vec<PimImportItem>,
}
