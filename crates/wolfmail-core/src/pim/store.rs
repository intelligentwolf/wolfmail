//! Calendar & contacts store: its own SQLite file (`pim.sqlite3`) next to the mail store, so the two schemas
//! migrate independently. Events and contacts keep their full wire JSON plus the few columns queries need.
//! Change tracking: `dirty=1` = local edit not yet pushed; `deleted=1` = local delete not yet pushed (the row is
//! hidden from every read and removed once the server confirms).
use super::model::*;
use crate::db::now_ms;
use crate::error::{ApiError, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE pim_accounts (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        email TEXT,
        url TEXT,
        username TEXT,
        enabled INTEGER NOT NULL DEFAULT 1,
        status TEXT NOT NULL DEFAULT 'ok',
        status_message TEXT,
        last_sync_at INTEGER,
        -- per-backend sync bookmarks (DAV home-set URLs, People sync token …) as JSON
        state TEXT NOT NULL DEFAULT '{}',
        created_at INTEGER NOT NULL
    );
    CREATE TABLE calendars (
        id TEXT PRIMARY KEY,
        account_id TEXT NOT NULL REFERENCES pim_accounts(id) ON DELETE CASCADE,
        remote_id TEXT NOT NULL,
        name TEXT NOT NULL,
        color TEXT NOT NULL,
        visible INTEGER NOT NULL DEFAULT 1,
        read_only INTEGER NOT NULL DEFAULT 0,
        is_default INTEGER NOT NULL DEFAULT 0,
        sort_order INTEGER NOT NULL DEFAULT 0,
        sync_token TEXT,
        ctag TEXT,
        UNIQUE(account_id, remote_id)
    );
    CREATE TABLE events (
        id TEXT PRIMARY KEY,
        calendar_id TEXT NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
        uid TEXT NOT NULL,
        recurrence_id INTEGER,
        -- DAV: resource href (master and overrides share it); Google: event id; Graph: occurrence/event id
        remote_id TEXT,
        -- Graph series master id for occurrences (edit the whole series)
        series_remote_id TEXT,
        etag TEXT,
        start INTEGER NOT NULL,
        end_ INTEGER NOT NULL,
        recurring INTEGER NOT NULL DEFAULT 0,
        json TEXT NOT NULL,
        dirty INTEGER NOT NULL DEFAULT 0,
        deleted INTEGER NOT NULL DEFAULT 0,
        updated_at INTEGER NOT NULL
    );
    CREATE UNIQUE INDEX events_uid ON events(calendar_id, uid, IFNULL(recurrence_id, -1));
    CREATE INDEX events_range ON events(calendar_id, start, end_);
    CREATE INDEX events_remote ON events(calendar_id, remote_id);
    CREATE TABLE address_books (
        id TEXT PRIMARY KEY,
        account_id TEXT NOT NULL REFERENCES pim_accounts(id) ON DELETE CASCADE,
        remote_id TEXT NOT NULL,
        name TEXT NOT NULL,
        read_only INTEGER NOT NULL DEFAULT 0,
        sync_token TEXT,
        ctag TEXT,
        UNIQUE(account_id, remote_id)
    );
    CREATE TABLE contacts (
        id TEXT PRIMARY KEY,
        book_id TEXT NOT NULL REFERENCES address_books(id) ON DELETE CASCADE,
        uid TEXT NOT NULL,
        remote_id TEXT,
        etag TEXT,
        sort_name TEXT NOT NULL,
        json TEXT NOT NULL,
        dirty INTEGER NOT NULL DEFAULT 0,
        deleted INTEGER NOT NULL DEFAULT 0,
        updated_at INTEGER NOT NULL,
        UNIQUE(book_id, uid)
    );
    CREATE INDEX contacts_remote ON contacts(book_id, remote_id);
    CREATE VIRTUAL TABLE contacts_fts USING fts5(contact_id UNINDEXED, body, tokenize='unicode61 remove_diacritics 2');
    CREATE TABLE fired_reminders (
        event_id TEXT NOT NULL,
        occurrence_start INTEGER NOT NULL,
        minutes INTEGER NOT NULL,
        PRIMARY KEY(event_id, occurrence_start, minutes)
    );
    "#,
];

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A full "not found" sentence (ApiError::not_found appends its own wording).
fn gone(msg: &str) -> ApiError {
    ApiError::new(crate::error::ErrorKind::NotFound, msg)
}

// ── accounts ────────────────────────────────────────────────────────────────

fn account_row(r: &rusqlite::Row) -> rusqlite::Result<PimAccount> {
    Ok(PimAccount {
        id: r.get(0)?,
        kind: PimKind::parse(&r.get::<_, String>(1)?),
        name: r.get(2)?,
        email: r.get(3)?,
        url: r.get(4)?,
        username: r.get(5)?,
        enabled: r.get::<_, i64>(6)? != 0,
        status: r.get(7)?,
        status_message: r.get(8)?,
        last_sync_at: r.get(9)?,
    })
}

const ACCOUNT_COLS: &str = "id, kind, name, email, url, username, enabled, status, status_message, last_sync_at";

pub fn accounts(c: &Connection) -> Result<Vec<PimAccount>> {
    let mut st = c.prepare(&format!("SELECT {ACCOUNT_COLS} FROM pim_accounts ORDER BY created_at"))?;
    let rows = st.query_map([], account_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn account(c: &Connection, id: &str) -> Result<PimAccount> {
    c.query_row(&format!("SELECT {ACCOUNT_COLS} FROM pim_accounts WHERE id=?1"), [id], account_row)
        .optional()?
        .ok_or_else(|| gone("That calendar/contacts account no longer exists."))
}

pub fn insert_account(c: &Connection, a: &PimAccount) -> Result<()> {
    c.execute(
        "INSERT INTO pim_accounts(id, kind, name, email, url, username, enabled, status, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,'ok',?8)",
        params![a.id, a.kind.as_str(), a.name, a.email, a.url, a.username, a.enabled as i64, now_ms()],
    )?;
    Ok(())
}

pub fn set_status(c: &Connection, id: &str, status: &str, msg: Option<&str>) -> Result<()> {
    let synced = if status == "ok" { Some(now_ms()) } else { None };
    c.execute(
        "UPDATE pim_accounts SET status=?2, status_message=?3, last_sync_at=COALESCE(?4, last_sync_at) WHERE id=?1",
        params![id, status, msg, synced],
    )?;
    Ok(())
}

pub fn state(c: &Connection, id: &str) -> Result<serde_json::Value> {
    let s: String = c.query_row("SELECT state FROM pim_accounts WHERE id=?1", [id], |r| r.get(0))?;
    Ok(serde_json::from_str(&s).unwrap_or(serde_json::json!({})))
}

pub fn set_state(c: &Connection, id: &str, v: &serde_json::Value) -> Result<()> {
    c.execute("UPDATE pim_accounts SET state=?2 WHERE id=?1", params![id, v.to_string()])?;
    Ok(())
}

// ── calendars ───────────────────────────────────────────────────────────────

const CAL_COLS: &str = "id, account_id, name, color, visible, read_only, is_default, sort_order";

fn cal_row(r: &rusqlite::Row) -> rusqlite::Result<Calendar> {
    Ok(Calendar {
        id: r.get(0)?,
        account_id: r.get(1)?,
        name: r.get(2)?,
        color: r.get(3)?,
        visible: r.get::<_, i64>(4)? != 0,
        read_only: r.get::<_, i64>(5)? != 0,
        is_default: r.get::<_, i64>(6)? != 0,
        sort_order: r.get(7)?,
    })
}

pub fn calendars(c: &Connection) -> Result<Vec<Calendar>> {
    let mut st = c.prepare(&format!(
        "SELECT {CAL_COLS} FROM calendars WHERE account_id IN (SELECT id FROM pim_accounts WHERE enabled=1) ORDER BY sort_order, name COLLATE NOCASE"
    ))?;
    let rows = st.query_map([], cal_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn calendar(c: &Connection, id: &str) -> Result<Calendar> {
    c.query_row(&format!("SELECT {CAL_COLS} FROM calendars WHERE id=?1"), [id], cal_row)
        .optional()?
        .ok_or_else(|| gone("That calendar no longer exists."))
}

pub fn calendar_remote(c: &Connection, id: &str) -> Result<(String, String, Option<String>, Option<String>)> {
    Ok(c.query_row("SELECT account_id, remote_id, sync_token, ctag FROM calendars WHERE id=?1", [id], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })?)
}

pub fn calendars_of(c: &Connection, account_id: &str) -> Result<Vec<(String, String)>> {
    let mut st = c.prepare("SELECT id, remote_id FROM calendars WHERE account_id=?1")?;
    let rows = st.query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Insert or refresh a server calendar; local choices (colour override, visibility) are kept on refresh.
pub fn upsert_calendar(c: &Connection, account_id: &str, remote_id: &str, name: &str, color: &str, read_only: bool, is_default: bool) -> Result<String> {
    if let Some(id) = c
        .query_row("SELECT id FROM calendars WHERE account_id=?1 AND remote_id=?2", params![account_id, remote_id], |r| r.get::<_, String>(0))
        .optional()?
    {
        c.execute("UPDATE calendars SET name=?2, read_only=?3, is_default=?4 WHERE id=?1", params![id, name, read_only as i64, is_default as i64])?;
        return Ok(id);
    }
    let id = new_id();
    let order: i64 = c.query_row("SELECT COALESCE(MAX(sort_order),0)+1 FROM calendars", [], |r| r.get(0))?;
    c.execute(
        "INSERT INTO calendars(id, account_id, remote_id, name, color, read_only, is_default, sort_order) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![id, account_id, remote_id, name, color, read_only as i64, is_default as i64, order],
    )?;
    Ok(id)
}

pub fn set_calendar_sync(c: &Connection, id: &str, token: Option<&str>, ctag: Option<&str>) -> Result<()> {
    c.execute("UPDATE calendars SET sync_token=?2, ctag=?3 WHERE id=?1", params![id, token, ctag])?;
    Ok(())
}

/// Remove calendars the server no longer lists.
pub fn prune_calendars(c: &Connection, account_id: &str, keep_remote: &[String]) -> Result<()> {
    for (id, remote) in calendars_of(c, account_id)? {
        if !keep_remote.contains(&remote) {
            c.execute("DELETE FROM calendars WHERE id=?1", [id])?;
        }
    }
    Ok(())
}

// ── events ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct EventRow {
    pub event: Event,
    pub remote_id: Option<String>,
    pub series_remote_id: Option<String>,
    pub etag: Option<String>,
    pub dirty: bool,
    pub deleted: bool,
}

fn event_row(r: &rusqlite::Row) -> rusqlite::Result<EventRow> {
    let json: String = r.get(0)?;
    let mut event: Event = serde_json::from_str(&json).map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?;
    event.id = r.get(1)?;
    Ok(EventRow { event, remote_id: r.get(2)?, series_remote_id: r.get(3)?, etag: r.get(4)?, dirty: r.get::<_, i64>(5)? != 0, deleted: r.get::<_, i64>(6)? != 0 })
}

const EV_COLS: &str = "json, id, remote_id, series_remote_id, etag, dirty, deleted";

pub fn event(c: &Connection, id: &str) -> Result<EventRow> {
    c.query_row(&format!("SELECT {EV_COLS} FROM events WHERE id=?1"), [id], event_row)
        .optional()?
        .ok_or_else(|| gone("That event no longer exists."))
}

/// Masters and single events that can produce an occurrence overlapping [from, to), plus overrides in range.
pub fn events_for_range(c: &Connection, calendar_ids: &[String], from: i64, to: i64) -> Result<Vec<EventRow>> {
    if calendar_ids.is_empty() {
        return Ok(vec![]);
    }
    let marks = vec!["?"; calendar_ids.len()].join(",");
    let sql = format!(
        "SELECT {EV_COLS} FROM events WHERE deleted=0 AND calendar_id IN ({marks})
         AND ((recurring=1 AND start < ?{a}) OR (start < ?{a} AND end_ > ?{b}) OR (recurrence_id IS NOT NULL AND recurrence_id < ?{a} AND recurrence_id >= ?{b} - 86400000*7))",
        a = calendar_ids.len() + 1,
        b = calendar_ids.len() + 2
    );
    let mut st = c.prepare(&sql)?;
    let mut args: Vec<rusqlite::types::Value> = calendar_ids.iter().map(|s| s.clone().into()).collect();
    args.push(to.into());
    args.push(from.into());
    let rows = st.query_map(rusqlite::params_from_iter(args), event_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// All rows (master + overrides) of one UID in one calendar — one CalDAV resource.
pub fn events_by_uid(c: &Connection, calendar_id: &str, uid: &str) -> Result<Vec<EventRow>> {
    let mut st = c.prepare(&format!("SELECT {EV_COLS} FROM events WHERE calendar_id=?1 AND uid=?2 ORDER BY recurrence_id IS NOT NULL, recurrence_id"))?;
    let rows = st.query_map(params![calendar_id, uid], event_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn events_by_remote(c: &Connection, calendar_id: &str, remote_id: &str) -> Result<Vec<EventRow>> {
    let mut st = c.prepare(&format!("SELECT {EV_COLS} FROM events WHERE calendar_id=?1 AND remote_id=?2"))?;
    let rows = st.query_map(params![calendar_id, remote_id], event_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn find_by_uid_any(c: &Connection, uid: &str, recurrence_id: Option<i64>) -> Result<Option<EventRow>> {
    Ok(c.query_row(
        &format!("SELECT {EV_COLS} FROM events WHERE uid=?1 AND IFNULL(recurrence_id,-1)=IFNULL(?2,-1) AND deleted=0 LIMIT 1"),
        params![uid, recurrence_id],
        event_row,
    )
    .optional()?)
}

pub fn dirty_events(c: &Connection, account_id: &str) -> Result<Vec<EventRow>> {
    let mut st = c.prepare(&format!(
        "SELECT {EV_COLS} FROM events WHERE (dirty=1 OR deleted=1) AND calendar_id IN (SELECT id FROM calendars WHERE account_id=?1)"
    ))?;
    let rows = st.query_map([account_id], event_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Write an event (insert or replace by id). Keeps `json` and the query columns in step.
pub fn put_event(c: &Connection, e: &Event, remote_id: Option<&str>, series_remote_id: Option<&str>, etag: Option<&str>, dirty: bool) -> Result<()> {
    let mut stored = e.clone();
    stored.read_only = false;
    let json = serde_json::to_string(&stored)?;
    c.execute(
        "INSERT INTO events(id, calendar_id, uid, recurrence_id, remote_id, series_remote_id, etag, start, end_, recurring, json, dirty, deleted, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,0,?13)
         ON CONFLICT(id) DO UPDATE SET calendar_id=excluded.calendar_id, uid=excluded.uid, recurrence_id=excluded.recurrence_id,
           remote_id=excluded.remote_id, series_remote_id=excluded.series_remote_id, etag=excluded.etag, start=excluded.start,
           end_=excluded.end_, recurring=excluded.recurring, json=excluded.json, dirty=excluded.dirty, deleted=0, updated_at=excluded.updated_at",
        params![
            e.id,
            e.calendar_id,
            e.uid,
            e.recurrence_id,
            remote_id,
            series_remote_id,
            etag,
            e.start,
            e.end.max(e.start),
            e.rrule.is_some() as i64,
            json,
            dirty as i64,
            now_ms()
        ],
    )?;
    Ok(())
}

/// Server copy of an event arrived: match an existing row by (calendar, uid, recurrence-id) so local ids stay
/// stable, and never overwrite a local edit that hasn't been pushed yet.
pub fn upsert_remote_event(c: &Connection, mut e: Event, remote_id: &str, series_remote_id: Option<&str>, etag: Option<&str>) -> Result<String> {
    let existing: Option<(String, i64, i64)> = c
        .query_row(
            "SELECT id, dirty, deleted FROM events WHERE calendar_id=?1 AND uid=?2 AND IFNULL(recurrence_id,-1)=IFNULL(?3,-1)",
            params![e.calendar_id, e.uid, e.recurrence_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((id, dirty, deleted)) = existing {
        if dirty != 0 || deleted != 0 {
            return Ok(id);
        }
        e.id = id;
    } else if e.id.is_empty() {
        e.id = new_id();
    }
    put_event(c, &e, Some(remote_id), series_remote_id, etag, false)?;
    Ok(e.id)
}

pub fn mark_event_deleted(c: &Connection, id: &str) -> Result<()> {
    c.execute("UPDATE events SET deleted=1, updated_at=?2 WHERE id=?1", params![id, now_ms()])?;
    Ok(())
}

pub fn remove_event(c: &Connection, id: &str) -> Result<()> {
    c.execute("DELETE FROM events WHERE id=?1", [id])?;
    c.execute("DELETE FROM fired_reminders WHERE event_id=?1", [id])?;
    Ok(())
}

pub fn clear_event_dirty(c: &Connection, id: &str, remote_id: Option<&str>, etag: Option<&str>) -> Result<()> {
    c.execute(
        "UPDATE events SET dirty=0, remote_id=COALESCE(?2, remote_id), etag=COALESCE(?3, etag) WHERE id=?1",
        params![id, remote_id, etag],
    )?;
    Ok(())
}

/// Remove rows the server no longer has (clean rows only — pending local edits survive).
pub fn remove_events_by_remote(c: &Connection, calendar_id: &str, remote_id: &str) -> Result<()> {
    c.execute("DELETE FROM events WHERE calendar_id=?1 AND remote_id=?2 AND dirty=0 AND deleted=0", params![calendar_id, remote_id])?;
    Ok(())
}

pub fn remote_ids(c: &Connection, calendar_id: &str) -> Result<Vec<(String, Option<String>)>> {
    let mut st = c.prepare("SELECT DISTINCT remote_id, etag FROM events WHERE calendar_id=?1 AND remote_id IS NOT NULL")?;
    let rows = st.query_map([calendar_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn clear_calendar(c: &Connection, calendar_id: &str) -> Result<()> {
    c.execute("DELETE FROM events WHERE calendar_id=?1 AND dirty=0 AND deleted=0", [calendar_id])?;
    Ok(())
}

// ── address books & contacts ────────────────────────────────────────────────

pub fn address_books(c: &Connection) -> Result<Vec<AddressBook>> {
    let mut st = c.prepare(
        "SELECT id, account_id, name, read_only FROM address_books WHERE account_id IN (SELECT id FROM pim_accounts WHERE enabled=1) ORDER BY name COLLATE NOCASE",
    )?;
    let rows = st
        .query_map([], |r| Ok(AddressBook { id: r.get(0)?, account_id: r.get(1)?, name: r.get(2)?, read_only: r.get::<_, i64>(3)? != 0 }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn address_book(c: &Connection, id: &str) -> Result<AddressBook> {
    c.query_row("SELECT id, account_id, name, read_only FROM address_books WHERE id=?1", [id], |r| {
        Ok(AddressBook { id: r.get(0)?, account_id: r.get(1)?, name: r.get(2)?, read_only: r.get::<_, i64>(3)? != 0 })
    })
    .optional()?
    .ok_or_else(|| gone("That address book no longer exists."))
}

pub fn book_remote(c: &Connection, id: &str) -> Result<(String, String, Option<String>, Option<String>)> {
    Ok(c.query_row("SELECT account_id, remote_id, sync_token, ctag FROM address_books WHERE id=?1", [id], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })?)
}

pub fn books_of(c: &Connection, account_id: &str) -> Result<Vec<(String, String)>> {
    let mut st = c.prepare("SELECT id, remote_id FROM address_books WHERE account_id=?1")?;
    let rows = st.query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn upsert_book(c: &Connection, account_id: &str, remote_id: &str, name: &str, read_only: bool) -> Result<String> {
    if let Some(id) = c
        .query_row("SELECT id FROM address_books WHERE account_id=?1 AND remote_id=?2", params![account_id, remote_id], |r| r.get::<_, String>(0))
        .optional()?
    {
        c.execute("UPDATE address_books SET name=?2, read_only=?3 WHERE id=?1", params![id, name, read_only as i64])?;
        return Ok(id);
    }
    let id = new_id();
    c.execute(
        "INSERT INTO address_books(id, account_id, remote_id, name, read_only) VALUES (?1,?2,?3,?4,?5)",
        params![id, account_id, remote_id, name, read_only as i64],
    )?;
    Ok(id)
}

pub fn set_book_sync(c: &Connection, id: &str, token: Option<&str>, ctag: Option<&str>) -> Result<()> {
    c.execute("UPDATE address_books SET sync_token=?2, ctag=?3 WHERE id=?1", params![id, token, ctag])?;
    Ok(())
}

pub fn prune_books(c: &Connection, account_id: &str, keep_remote: &[String]) -> Result<()> {
    for (id, remote) in books_of(c, account_id)? {
        if !keep_remote.contains(&remote) {
            c.execute("DELETE FROM address_books WHERE id=?1", [id])?;
        }
    }
    Ok(())
}

pub struct ContactRow {
    pub contact: Contact,
    pub remote_id: Option<String>,
    pub etag: Option<String>,
    pub dirty: bool,
    pub deleted: bool,
}

fn contact_row(r: &rusqlite::Row) -> rusqlite::Result<ContactRow> {
    let json: String = r.get(0)?;
    let mut contact: Contact = serde_json::from_str(&json).map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?;
    contact.id = r.get(1)?;
    Ok(ContactRow { contact, remote_id: r.get(2)?, etag: r.get(3)?, dirty: r.get::<_, i64>(4)? != 0, deleted: r.get::<_, i64>(5)? != 0 })
}

const CT_COLS: &str = "c.json, c.id, c.remote_id, c.etag, c.dirty, c.deleted";

pub fn contact(c: &Connection, id: &str) -> Result<ContactRow> {
    c.query_row(&format!("SELECT {CT_COLS} FROM contacts c WHERE c.id=?1"), [id], contact_row)
        .optional()?
        .ok_or_else(|| gone("That contact no longer exists."))
}

fn sort_name(ct: &Contact) -> String {
    let n = if !ct.full_name.trim().is_empty() {
        ct.full_name.clone()
    } else if !ct.org.is_empty() {
        ct.org.clone()
    } else {
        ct.emails.first().map(|e| e.value.clone()).unwrap_or_default()
    };
    n.to_lowercase()
}

fn fts_body(ct: &Contact) -> String {
    let mut parts = vec![ct.full_name.clone(), ct.given_name.clone(), ct.family_name.clone(), ct.org.clone(), ct.title.clone(), ct.notes.clone()];
    parts.extend(ct.emails.iter().map(|e| e.value.replace(['@', '.'], " ") + " " + &e.value));
    parts.extend(ct.phones.iter().map(|p| p.value.clone()));
    parts.extend(ct.categories.iter().cloned());
    parts.join(" ")
}

pub fn put_contact(c: &Connection, ct: &Contact, remote_id: Option<&str>, etag: Option<&str>, dirty: bool) -> Result<()> {
    let mut stored = ct.clone();
    stored.read_only = false;
    c.execute(
        "INSERT INTO contacts(id, book_id, uid, remote_id, etag, sort_name, json, dirty, deleted, updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,0,?9)
         ON CONFLICT(id) DO UPDATE SET book_id=excluded.book_id, uid=excluded.uid, remote_id=excluded.remote_id, etag=excluded.etag,
           sort_name=excluded.sort_name, json=excluded.json, dirty=excluded.dirty, deleted=0, updated_at=excluded.updated_at",
        params![ct.id, ct.book_id, ct.uid, remote_id, etag, sort_name(ct), serde_json::to_string(&stored)?, dirty as i64, now_ms()],
    )?;
    c.execute("DELETE FROM contacts_fts WHERE contact_id=?1", [&ct.id])?;
    c.execute("INSERT INTO contacts_fts(contact_id, body) VALUES (?1, ?2)", params![ct.id, fts_body(ct)])?;
    Ok(())
}

pub fn upsert_remote_contact(c: &Connection, mut ct: Contact, remote_id: &str, etag: Option<&str>) -> Result<String> {
    let existing: Option<(String, i64, i64)> = c
        .query_row(
            "SELECT id, dirty, deleted FROM contacts WHERE book_id=?1 AND (uid=?2 OR remote_id=?3)",
            params![ct.book_id, ct.uid, remote_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((id, dirty, deleted)) = existing {
        if dirty != 0 || deleted != 0 {
            return Ok(id);
        }
        ct.id = id;
    } else {
        ct.id = new_id();
    }
    if ct.uid.is_empty() {
        ct.uid = remote_id.to_string();
    }
    put_contact(c, &ct, Some(remote_id), etag, false)?;
    Ok(ct.id)
}

pub fn remove_contact(c: &Connection, id: &str) -> Result<()> {
    c.execute("DELETE FROM contacts WHERE id=?1", [id])?;
    c.execute("DELETE FROM contacts_fts WHERE contact_id=?1", [id])?;
    Ok(())
}

pub fn remove_contacts_by_remote(c: &Connection, book_id: &str, remote_id: &str) -> Result<()> {
    let ids: Vec<String> = {
        let mut st = c.prepare("SELECT id FROM contacts WHERE book_id=?1 AND remote_id=?2 AND dirty=0 AND deleted=0")?;
        let v = st.query_map(params![book_id, remote_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        v
    };
    for id in ids {
        remove_contact(c, &id)?;
    }
    Ok(())
}

pub fn contact_remote_ids(c: &Connection, book_id: &str) -> Result<Vec<(String, Option<String>)>> {
    let mut st = c.prepare("SELECT remote_id, etag FROM contacts WHERE book_id=?1 AND remote_id IS NOT NULL")?;
    let rows = st.query_map([book_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn dirty_contacts(c: &Connection, account_id: &str) -> Result<Vec<ContactRow>> {
    let mut st = c.prepare(&format!(
        "SELECT {CT_COLS} FROM contacts c WHERE (c.dirty=1 OR c.deleted=1) AND c.book_id IN (SELECT id FROM address_books WHERE account_id=?1)"
    ))?;
    let rows = st.query_map([account_id], contact_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn clear_contact_dirty(c: &Connection, id: &str, remote_id: Option<&str>, etag: Option<&str>) -> Result<()> {
    c.execute(
        "UPDATE contacts SET dirty=0, remote_id=COALESCE(?2, remote_id), etag=COALESCE(?3, etag) WHERE id=?1",
        params![id, remote_id, etag],
    )?;
    Ok(())
}

pub fn mark_contact_deleted(c: &Connection, id: &str) -> Result<()> {
    c.execute("UPDATE contacts SET deleted=1, updated_at=?2 WHERE id=?1", params![id, now_ms()])?;
    Ok(())
}

/// Contacts in visible books, optionally filtered by one book and/or a search (FTS prefix match per word).
pub fn contacts_list(c: &Connection, book_id: Option<&str>, query: Option<&str>, limit: i64) -> Result<Vec<Contact>> {
    let fts = query.map(fts_query).filter(|q| !q.is_empty());
    let mut sql = format!(
        "SELECT {CT_COLS} FROM contacts c JOIN address_books b ON b.id=c.book_id
         WHERE c.deleted=0 AND b.account_id IN (SELECT id FROM pim_accounts WHERE enabled=1)"
    );
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(b) = book_id {
        args.push(b.to_string().into());
        sql.push_str(&format!(" AND c.book_id=?{}", args.len()));
    }
    if let Some(q) = &fts {
        args.push(q.clone().into());
        sql.push_str(&format!(" AND c.id IN (SELECT contact_id FROM contacts_fts WHERE contacts_fts MATCH ?{})", args.len()));
    }
    args.push(limit.into());
    sql.push_str(&format!(" ORDER BY c.sort_name LIMIT ?{}", args.len()));
    let mut st = c.prepare(&sql)?;
    let ro = read_only_books(c)?;
    let rows = st
        .query_map(rusqlite::params_from_iter(args), contact_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|mut r| {
            r.contact.read_only = ro.contains(&r.contact.book_id);
            r.contact
        })
        .collect();
    Ok(rows)
}

pub fn read_only_books(c: &Connection) -> Result<std::collections::HashSet<String>> {
    let mut st = c.prepare("SELECT id FROM address_books WHERE read_only=1")?;
    let v = st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
    Ok(v)
}

/// Turn free text into an FTS5 query: each word as a quoted prefix term, AND-ed.
pub fn fts_query(q: &str) -> String {
    q.split(|c: char| !c.is_alphanumeric() && c != '+')
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"*", w.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}

// ── reminders ───────────────────────────────────────────────────────────────

pub fn reminder_fired(c: &Connection, event_id: &str, occ: i64, minutes: i64) -> Result<bool> {
    Ok(c.query_row(
        "SELECT 1 FROM fired_reminders WHERE event_id=?1 AND occurrence_start=?2 AND minutes=?3",
        params![event_id, occ, minutes],
        |_| Ok(()),
    )
    .optional()?
    .is_some())
}

pub fn mark_reminder_fired(c: &Connection, event_id: &str, occ: i64, minutes: i64) -> Result<()> {
    c.execute("INSERT OR IGNORE INTO fired_reminders(event_id, occurrence_start, minutes) VALUES (?1,?2,?3)", params![event_id, occ, minutes])?;
    // Keep the table small: forget reminders for occurrences more than two days old.
    c.execute("DELETE FROM fired_reminders WHERE occurrence_start < ?1", [now_ms() - 2 * 86_400_000])?;
    Ok(())
}
