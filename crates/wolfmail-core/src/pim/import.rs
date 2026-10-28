//! Bring calendars and address books over from a Thunderbird/Betterbird profile (read-only: the profile is never
//! written; local.sqlite is opened read-only).
//! Sources:
//! - prefs.js keys as Betterbird writes them (Paul's profile, read from a copy): `calendar.registry.<id>.type`
//!   (caldav | storage | ics), `.uri`, `.name`, `.color`, `.username`, `.disabled`; `ldap_2.servers.<key>.carddav.url`,
//!   `.carddav.username`, `.description`, `.dirType` (102 = CardDAV).
//! - Google CalDAV collection URL "https://apidata.googleusercontent.com/caldav/v2/CALENDAR_ID/events" (Google
//!   CalDAV guide) — CALENDAR_ID is the Calendar API v3 calendar id. Google CardDAV lives under
//!   www.googleapis.com/carddav (Google CardDAV guide).
//! - Lightning storage (comm-central calendar/providers/storage): cal_events(cal_id, id, title, flags, event_start,
//!   event_end, event_start_tz, event_end_tz, recurrence_id, recurrence_id_tz), cal_properties(item_id, key, value,
//!   recurrence_id), cal_recurrence(item_id, icalString), cal_attendees(item_id, recurrence_id, icalString),
//!   cal_alarms(item_id, recurrence_id, icalString). Times are PRTime (µs since the epoch, `nativeTime`); `_tz` is a
//!   TZID, "UTC" or "floating" (calStorageHelpers.sys.mjs newDateTime). CAL_ITEM_FLAG: HAS_ATTENDEES=2,
//!   HAS_PROPERTIES=4, EVENT_ALLDAY=8, HAS_RECURRENCE=16, HAS_EXCEPTIONS=32, HAS_ALARMS=256, RECURRENCE_ID_ALLDAY=512.
use super::lines::{self, Component, Prop};
use super::model::*;
use super::{ical, store};
use crate::engine::Engine;
use crate::error::{ApiError, Result};
use crate::import::{base_dirs, parse_prefs, profiles_in, PrefValue};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

const FLAG_ALLDAY: i64 = 8;

/// Per Google user: (calendar id, colour) of the calendars shown in Betterbird, and whether contacts were used.
type GoogleImport = (Vec<(String, Option<String>)>, bool);

fn pref_str(p: &HashMap<String, PrefValue>, k: &str) -> Option<String> {
    match p.get(k)? {
        PrefValue::Str(s) => Some(s.clone()),
        PrefValue::Int(n) => Some(n.to_string()),
        PrefValue::Bool(b) => Some(b.to_string()),
    }
}

fn pref_bool(p: &HashMap<String, PrefValue>, k: &str) -> bool {
    matches!(p.get(k), Some(PrefValue::Bool(true))) || matches!(p.get(k), Some(PrefValue::Str(s)) if s == "true")
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes()).next().map(|(_, v)| v.into_owned()).unwrap_or_else(|| s.to_string())
}

/// Google CalDAV URL → Calendar API calendar id.
pub fn google_calendar_id(uri: &str) -> Option<String> {
    let u = url::Url::parse(uri).ok()?;
    if u.host_str()? != "apidata.googleusercontent.com" {
        return None;
    }
    let segs: Vec<&str> = u.path_segments()?.collect();
    let i = segs.iter().position(|s| *s == "v2")?;
    segs.get(i + 1).map(|s| percent_decode(s))
}

fn is_google_carddav(uri: &str) -> bool {
    url::Url::parse(uri).ok().and_then(|u| u.host_str().map(|h| h == "www.googleapis.com" && u.path().starts_with("/carddav"))).unwrap_or(false)
}

fn local_event_count(profile: &Path, cal_id: &str) -> i64 {
    let path = profile.join("calendar-data").join("local.sqlite");
    let Ok(c) = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else { return 0 };
    c.query_row("SELECT COUNT(*) FROM cal_events WHERE cal_id=?1 AND recurrence_id IS NULL", [cal_id], |r| r.get(0)).unwrap_or(0)
}

pub fn read_profile(profile: &Path) -> Result<PimImportCandidate> {
    let src = std::fs::read_to_string(profile.join("prefs.js")).map_err(|e| ApiError::invalid("profilePath", format!("No prefs.js in that folder ({e}).")))?;
    let p = parse_prefs(&src);
    let mut items = Vec::new();
    let reg_ids: BTreeSet<String> = p
        .keys()
        .filter_map(|k| k.strip_prefix("calendar.registry.").and_then(|r| r.split_once('.')).map(|(id, _)| id.to_string()))
        .collect();
    for id in reg_ids {
        let key = |f: &str| format!("calendar.registry.{id}.{f}");
        let kind = pref_str(&p, &key("type")).unwrap_or_default();
        let name = pref_str(&p, &key("name")).unwrap_or_else(|| "Calendar".into());
        let enabled = !pref_bool(&p, &key("disabled"));
        let color = pref_str(&p, &key("color")).filter(|c| c.starts_with('#') && c.len() >= 7).map(|c| c[..7].to_string());
        let uri = pref_str(&p, &key("uri"));
        match kind.as_str() {
            "caldav" => {
                let Some(u) = uri else { continue };
                let google = google_calendar_id(&u).is_some();
                items.push(PimImportItem {
                    kind: if google { "google-calendar".into() } else { "caldav".into() },
                    name,
                    url: Some(u),
                    username: pref_str(&p, &key("username")),
                    color,
                    enabled,
                    local_events: 0,
                });
            }
            "storage" => items.push(PimImportItem {
                kind: "local-calendar".into(),
                name,
                url: Some(id.clone()),
                username: None,
                color,
                enabled,
                local_events: local_event_count(profile, &id),
            }),
            _ => {}
        }
    }
    let book_keys: BTreeSet<String> = p
        .keys()
        .filter_map(|k| k.strip_prefix("ldap_2.servers.").and_then(|r| r.split_once('.')).map(|(id, _)| id.to_string()))
        .collect();
    for k in book_keys {
        let Some(url) = pref_str(&p, &format!("ldap_2.servers.{k}.carddav.url")) else { continue };
        let google = is_google_carddav(&url);
        items.push(PimImportItem {
            kind: if google { "google-contacts".into() } else { "carddav".into() },
            name: pref_str(&p, &format!("ldap_2.servers.{k}.description")).unwrap_or_else(|| "Address book".into()),
            url: Some(url),
            username: pref_str(&p, &format!("ldap_2.servers.{k}.carddav.username")),
            color: None,
            enabled: true,
            local_events: 0,
        });
    }
    Ok(PimImportCandidate { profile_path: profile.to_string_lossy().to_string(), items })
}

pub fn scan(profile_path: Option<&str>) -> Result<Vec<PimImportCandidate>> {
    if let Some(p) = profile_path {
        return Ok(vec![read_profile(Path::new(p))?]);
    }
    let mut out = Vec::new();
    for base in base_dirs() {
        for prof in profiles_in(&base) {
            if let Ok(c) = read_profile(&prof) {
                if !c.items.is_empty() {
                    out.push(c);
                }
            }
        }
    }
    Ok(out)
}

/// PRTime → unix ms, interpreted per Lightning's `_tz` column.
fn prtime(us: i64, tz: Option<&str>, date_only: bool) -> (i64, String) {
    let ms = us / 1000;
    if date_only {
        let d = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).unwrap_or_default().date_naive();
        return (d.and_hms_opt(0, 0, 0).unwrap_or_default().and_utc().timestamp_millis(), String::new());
    }
    match tz {
        Some("floating") | None => {
            // Floating: nativeTime holds the wall-clock time as if it were UTC.
            let naive = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms).unwrap_or_default().naive_utc();
            (ical::local_to_ms(naive, &ical::Zone::Local), String::new())
        }
        Some(z) => (ms, z.parse::<chrono_tz::Tz>().map(|t| t.name().to_string()).unwrap_or_default()),
    }
}

/// Read every event of one Lightning local calendar into `Event`s for `calendar_id`.
pub fn local_events(profile: &Path, lightning_cal: &str, calendar_id: &str) -> Result<Vec<Event>> {
    let path = profile.join("calendar-data").join("local.sqlite");
    let c = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| ApiError::storage(format!("Could not open the profile's calendar store: {e}")))?;
    let mut st = c.prepare(
        "SELECT id, title, flags, event_start, event_end, event_start_tz, event_end_tz, recurrence_id, recurrence_id_tz FROM cal_events WHERE cal_id=?1",
    )?;
    let rows = st
        .query_map([lightning_cal], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, Option<String>>(8)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for (id, title, flags, s, e, stz, etz, rid, rtz) in rows {
        let Some(s) = s else { continue };
        let all_day = flags & FLAG_ALLDAY != 0;
        let (start, tz) = prtime(s, stz.as_deref(), all_day);
        let (end, _) = e.map(|x| prtime(x, etz.as_deref(), all_day)).unwrap_or((start, String::new()));
        // Properties, attendees, alarms, recurrence as iCalendar lines → one VEVENT, parsed like any other.
        let mut ve = Component::new("VEVENT");
        ve.push(Prop::new("UID", lines::escape(&id)));
        let ridv = rid;
        let props: Vec<(String, Vec<u8>)> = {
            let mut q = c.prepare("SELECT key, value FROM cal_properties WHERE cal_id=?1 AND item_id=?2 AND IFNULL(recurrence_id,-1)=IFNULL(?3,-1)")?;
            let v = q
                .query_map(rusqlite::params![lightning_cal, id, ridv], |r| {
                    let val: rusqlite::types::Value = r.get(1)?;
                    let bytes = match val {
                        rusqlite::types::Value::Text(t) => t.into_bytes(),
                        rusqlite::types::Value::Blob(b) => b,
                        rusqlite::types::Value::Integer(i) => i.to_string().into_bytes(),
                        rusqlite::types::Value::Real(f) => f.to_string().into_bytes(),
                        rusqlite::types::Value::Null => Vec::new(),
                    };
                    Ok((r.get::<_, String>(0)?, bytes))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            v
        };
        for (k, v) in props {
            if matches!(k.as_str(), "DESCRIPTION" | "LOCATION" | "STATUS" | "SEQUENCE" | "CATEGORIES") {
                ve.push_text(&k, &String::from_utf8_lossy(&v));
            }
        }
        for table in ["cal_attendees", "cal_recurrence"] {
            let rec = table == "cal_recurrence";
            let sql = if rec {
                "SELECT icalString FROM cal_recurrence WHERE cal_id=?1 AND item_id=?2".to_string()
            } else {
                format!("SELECT icalString FROM {table} WHERE cal_id=?1 AND item_id=?2 AND IFNULL(recurrence_id,-1)=IFNULL(?3,-1)")
            };
            let mut q = c.prepare(&sql)?;
            let vals: Vec<String> = if rec {
                if ridv.is_some() {
                    vec![]
                } else {
                    q.query_map(rusqlite::params![lightning_cal, id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?
                }
            } else {
                q.query_map(rusqlite::params![lightning_cal, id, ridv], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?
            };
            for v in vals {
                for l in v.lines() {
                    if let Some(p) = lines::parse_line(l.trim_end_matches('\r')) {
                        ve.push(p);
                    }
                }
            }
        }
        let mut q = c.prepare("SELECT icalString FROM cal_alarms WHERE cal_id=?1 AND item_id=?2 AND IFNULL(recurrence_id,-1)=IFNULL(?3,-1)")?;
        let alarms: Vec<String> = q.query_map(rusqlite::params![lightning_cal, id, ridv], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        for a in alarms {
            ve.children.extend(lines::parse(&a).into_iter().filter(|x| x.name == "VALARM"));
        }
        ve.push(Prop::new("DTSTART", ical::fmt_utc(start)));
        let Some(mut ev) = ical::event_from_vevent(&ve, &[], calendar_id) else { continue };
        ev.title = title.unwrap_or_default();
        ev.start = start;
        ev.end = end.max(start);
        ev.all_day = all_day;
        ev.timezone = tz;
        if let Some(r) = rid {
            ev.recurrence_id = Some(prtime(r, rtz.as_deref(), flags & 512 != 0).0);
        }
        out.push(ev);
    }
    Ok(out)
}

impl Engine {
    pub async fn pim_import_scan(&self, profile_path: Option<String>) -> Result<Vec<PimImportCandidate>> {
        tokio::task::spawn_blocking(move || scan(profile_path.as_deref())).await?
    }

    /// Create the accounts the chosen items need. Accounts that need a password or a Google/Microsoft sign-in
    /// are created in the "authError" state with a message saying so; local Lightning calendars are copied now.
    /// Returns the accounts created or reused.
    pub async fn pim_import_apply(&self, profile_path: String, items: Vec<PimImportItem>) -> Result<Vec<PimAccount>> {
        let profile = std::path::PathBuf::from(&profile_path);
        let mut touched: Vec<String> = Vec::new();
        // Google: one account per Google user, remembering which calendars were shown and their colours.
        let mut google: HashMap<String, GoogleImport> = HashMap::new();
        let mut dav: HashMap<(String, String), Vec<&PimImportItem>> = HashMap::new();
        for it in &items {
            match it.kind.as_str() {
                "google-calendar" => {
                    let user = it.username.clone().unwrap_or_default().to_ascii_lowercase();
                    if let Some(cid) = it.url.as_deref().and_then(google_calendar_id) {
                        if it.enabled {
                            google.entry(user).or_default().0.push((cid, it.color.clone()));
                        } else {
                            google.entry(user).or_default();
                        }
                    }
                }
                "google-contacts" => {
                    google.entry(it.username.clone().unwrap_or_default().to_ascii_lowercase()).or_default().1 = true;
                }
                "caldav" | "carddav" => {
                    let origin = it.url.as_deref().and_then(|u| url::Url::parse(u).ok()).map(|u| u.origin().ascii_serialization()).unwrap_or_default();
                    dav.entry((origin, it.username.clone().unwrap_or_default())).or_default().push(it);
                }
                _ => {}
            }
        }
        for (email, (cals, _)) in google {
            if email.is_empty() {
                continue;
            }
            let state = serde_json::json!({ "import": { "calendars": cals.iter().map(|(id, c)| serde_json::json!({ "id": id, "color": c })).collect::<Vec<_>>() } });
            let id = self.import_account(PimKind::Google, &email, Some(&email), None, None, state, "Sign in with Google to sync these calendars and contacts.").await?;
            touched.push(id);
        }
        for ((origin, user), its) in dav {
            let url = its.iter().find_map(|i| i.url.clone()).unwrap_or(origin.clone());
            let name = url::Url::parse(&origin).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_else(|| "Calendar server".into());
            let id = self.import_account(PimKind::Dav, &name, None, Some(&url), Some(&user), serde_json::json!({}), "Enter the password for this calendar/contacts server.").await?;
            touched.push(id);
        }
        // Local Lightning calendars → new calendars in "On this computer".
        for it in items.iter().filter(|i| i.kind == "local-calendar") {
            let lightning_id = it.url.clone().unwrap_or_default();
            let (name, color, enabled) = (it.name.clone(), it.color.clone().unwrap_or_else(|| "#2563eb".into()), it.enabled);
            let prof = profile.clone();
            let n = self
                .pim()
                .store
                .call(move |c| {
                    let local = store::accounts(c)?.into_iter().find(|a| a.kind == PimKind::Local).ok_or_else(|| ApiError::internal("no local account"))?;
                    let cal_id = store::upsert_calendar(c, &local.id, &format!("import:{lightning_id}"), &name, &color, false, false)?;
                    c.execute("UPDATE calendars SET visible=?2 WHERE id=?1", rusqlite::params![cal_id, enabled as i64])?;
                    let evs = local_events(&prof, &lightning_id, &cal_id)?;
                    let n = evs.len();
                    for mut e in evs {
                        if let Some(existing) = store::find_by_uid_any(c, &e.uid, e.recurrence_id)? {
                            if existing.event.calendar_id == cal_id {
                                e.id = existing.event.id;
                            }
                        }
                        if e.id.is_empty() {
                            e.id = store::new_id();
                        }
                        store::put_event(c, &e, None, None, None, false)?;
                    }
                    Ok(n)
                })
                .await?;
            tracing::info!("imported {n} local events");
        }
        self.emit_pim_changed();
        let ids = touched;
        self.pim().store.call(move |c| Ok(store::accounts(c)?.into_iter().filter(|a| ids.contains(&a.id) || a.kind == PimKind::Local).collect())).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn import_account(&self, kind: PimKind, name: &str, email: Option<&str>, url: Option<&str>, username: Option<&str>, state: serde_json::Value, msg: &str) -> Result<String> {
        let (n, em, u, us, m) = (name.to_string(), email.map(String::from), url.map(String::from), username.map(String::from), msg.to_string());
        self.pim()
            .store
            .call(move |c| {
                // Reuse an account already pointing at the same place.
                let existing = store::accounts(c)?
                    .into_iter()
                    .find(|a| a.kind == kind && ((em.is_some() && a.email == em) || (u.is_some() && a.url == u && a.username == us)));
                if let Some(a) = existing {
                    return Ok(a.id);
                }
                let id = store::new_id();
                store::insert_account(
                    c,
                    &PimAccount { id: id.clone(), kind, name: n, email: em, url: u, username: us, enabled: true, status: "authError".into(), status_message: Some(m.clone()), last_sync_at: None },
                )?;
                store::set_status(c, &id, "authError", Some(&m))?;
                store::set_state(c, &id, &state)?;
                Ok(id)
            })
            .await
    }
}

/// Apply calendar colours/visibility remembered by an import, once, after the first Google sync.
pub fn apply_google_import(c: &rusqlite::Connection, account_id: &str) -> Result<()> {
    let st = store::state(c, account_id)?;
    let Some(imp) = st.get("import").and_then(|v| v.get("calendars")).and_then(|v| v.as_array()) else { return Ok(()) };
    let wanted: HashMap<String, Option<String>> =
        imp.iter().filter_map(|x| Some((x["id"].as_str()?.to_string(), x["color"].as_str().map(String::from)))).collect();
    for (cal_id, remote) in store::calendars_of(c, account_id)? {
        match wanted.get(&remote) {
            Some(color) => {
                c.execute("UPDATE calendars SET visible=1 WHERE id=?1", [&cal_id])?;
                if let Some(col) = color {
                    c.execute("UPDATE calendars SET color=?2 WHERE id=?1", rusqlite::params![cal_id, col])?;
                }
            }
            None => {
                c.execute("UPDATE calendars SET visible=0 WHERE id=?1", [&cal_id])?;
            }
        }
    }
    let mut st = st;
    if let Some(o) = st.as_object_mut() {
        o.remove("import");
    }
    store::set_state(c, account_id, &st)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_calendar_and_carddav_prefs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("prefs.js"),
            r##"user_pref("calendar.registry.a1.type", "caldav");
user_pref("calendar.registry.a1.uri", "https://apidata.googleusercontent.com/caldav/v2/abc%40group.calendar.google.com/events/");
user_pref("calendar.registry.a1.name", "Team");
user_pref("calendar.registry.a1.color", "#FAD165");
user_pref("calendar.registry.a1.username", "someone@example.test");
user_pref("calendar.registry.b2.type", "storage");
user_pref("calendar.registry.b2.name", "Home");
user_pref("calendar.registry.b2.disabled", true);
user_pref("calendar.registry.c3.type", "caldav");
user_pref("calendar.registry.c3.uri", "https://cloud.example.test/remote.php/dav/calendars/me/work/");
user_pref("calendar.registry.c3.username", "me");
user_pref("ldap_2.servers.AddressBook.carddav.url", "https://www.googleapis.com/carddav/v1/principals/someone@example.test/lists/default/");
user_pref("ldap_2.servers.AddressBook.carddav.username", "someone@example.test");
user_pref("ldap_2.servers.AddressBook.description", "Address Book");
"##,
        )
        .unwrap();
        let c = read_profile(dir.path()).unwrap();
        let g = c.items.iter().find(|i| i.kind == "google-calendar").unwrap();
        assert_eq!(google_calendar_id(g.url.as_deref().unwrap()).as_deref(), Some("abc@group.calendar.google.com"));
        assert_eq!(g.color.as_deref(), Some("#FAD165"));
        assert!(c.items.iter().any(|i| i.kind == "local-calendar" && !i.enabled));
        assert!(c.items.iter().any(|i| i.kind == "caldav"));
        assert!(c.items.iter().any(|i| i.kind == "google-contacts"));
    }

    /// Read-only check against a real profile COPY: `WOLFMAIL_IT_PROFILE=/path/to/copy cargo test real_profile`.
    #[test]
    fn real_profile_if_given() {
        let Ok(p) = std::env::var("WOLFMAIL_IT_PROFILE") else { return };
        let c = read_profile(Path::new(&p)).unwrap();
        for i in &c.items {
            eprintln!("IMPORT {:<16} enabled={} events={} name={:?} gcal={:?}", i.kind, i.enabled, i.local_events, i.name, i.url.as_deref().and_then(google_calendar_id).map(|s| s.len()));
        }
        assert!(!c.items.is_empty());
    }

    #[test]
    fn prtime_conversion() {
        // 2026-03-02T09:00:00Z in µs
        let us = 1_772_442_000_000_000i64;
        assert_eq!(prtime(us, Some("UTC"), false).0, 1_772_442_000_000);
        assert_eq!(prtime(us, Some("Europe/London"), false).1, "Europe/London");
        let (d, tz) = prtime(us, None, true);
        assert_eq!(d % 86_400_000, 0);
        assert!(tz.is_empty());
    }
}
