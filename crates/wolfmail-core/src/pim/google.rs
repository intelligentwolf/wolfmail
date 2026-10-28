//! Google: Calendar API v3 (calendars + events, read/write) and People API (contacts, read-only).
//! Sources (fetched 2026-10-28):
//! - calendarList.list `GET https://www.googleapis.com/calendar/v3/users/me/calendarList`
//! - events.list `GET …/calendars/{calendarId}/events`: syncToken (no timeMin/timeMax with it), showDeleted must be
//!   true with syncToken, 410 GONE = token expired → full sync; maxResults ≤ 2500; nextPageToken / nextSyncToken.
//! - event resource: id, iCalUID, status confirmed|tentative|cancelled, start/end {date | dateTime + timeZone},
//!   recurrence[] RRULE/EXDATE/RDATE lines (no DTSTART), recurringEventId, originalStartTime, attendees[]
//!   (email, displayName, responseStatus needsAction|declined|tentative|accepted, self, organizer, optional),
//!   reminders {useDefault, overrides[{method email|popup, minutes}] ≤ 5}, etag, sequence.
//! - events.instances `GET …/events/{eventId}/instances?originalStart=` (find one occurrence's id).
//! - insert/patch/delete on `…/calendars/{calendarId}/events[/{eventId}]` with sendUpdates=all so attendees get mail.
//! - People API `GET https://people.googleapis.com/v1/people/me/connections` (personFields required, pageSize ≤ 1000,
//!   requestSyncToken/syncToken, EXPIRED_SYNC_TOKEN → full sync) and `GET …/v1/otherContacts` (readMask
//!   names,emailAddresses,phoneNumbers; scope contacts.other.readonly).
use super::model::*;
use super::{http_status, lines, rest, store};
use crate::engine::Engine;
use crate::error::Result;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use reqwest::Method;
use serde_json::{json, Value};

const CAL: &str = "https://www.googleapis.com/calendar/v3";
const PEOPLE: &str = "https://people.googleapis.com/v1";

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

pub async fn sync(e: &Engine, acct: &PimAccount) -> Result<()> {
    let token = e.pim_token(acct).await?;
    let http = &e.inner.http;
    // Calendars
    let mut items: Vec<Value> = Vec::new();
    let mut page: Option<String> = None;
    loop {
        let mut u = format!("{CAL}/users/me/calendarList?maxResults=250");
        if let Some(p) = &page {
            u.push_str(&format!("&pageToken={}", enc(p)));
        }
        let (_, v) = rest(http, Method::GET, &u, &token, None, &[]).await?;
        items.extend(v["items"].as_array().cloned().unwrap_or_default());
        page = v["nextPageToken"].as_str().map(String::from);
        if page.is_none() {
            break;
        }
    }
    let acct_id = acct.id.clone();
    let cals = e
        .pim()
        .store
        .call(move |c| {
            let mut out = Vec::new();
            let mut keep = Vec::new();
            for it in items.iter().filter(|i| !i["deleted"].as_bool().unwrap_or(false)) {
                let rid = it["id"].as_str().unwrap_or_default().to_string();
                let name = it["summaryOverride"].as_str().or(it["summary"].as_str()).unwrap_or("Calendar").to_string();
                let color = it["backgroundColor"].as_str().unwrap_or("#2563eb").to_string();
                let role = it["accessRole"].as_str().unwrap_or("reader");
                let ro = !(role == "owner" || role == "writer");
                let id = store::upsert_calendar(c, &acct_id, &rid, &name, &color, ro, it["primary"].as_bool().unwrap_or(false))?;
                if it["hidden"].as_bool().unwrap_or(false) {
                    c.execute("UPDATE calendars SET visible=0 WHERE id=?1 AND sync_token IS NULL", [&id])?;
                }
                keep.push(rid.clone());
                out.push((id, rid));
            }
            store::prune_calendars(c, &acct_id, &keep)?;
            super::import::apply_google_import(c, &acct_id)?;
            Ok(out)
        })
        .await?;

    push(e, acct, &token).await?;
    for (cal_id, remote) in cals {
        pull_calendar(e, &token, &cal_id, &remote).await?;
    }
    pull_contacts(e, acct, &token).await?;
    Ok(())
}

// ── events: Google JSON ↔ Event ─────────────────────────────────────────────

fn parse_rfc3339(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis())
}

fn parse_gdate(s: &str) -> Option<i64> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().map(|d| d.and_time(NaiveTime::MIN).and_utc().timestamp_millis())
}

/// (ms, all_day, timeZone)
fn when(v: &Value) -> Option<(i64, bool, String)> {
    if let Some(d) = v["date"].as_str() {
        return Some((parse_gdate(d)?, true, String::new()));
    }
    let ms = parse_rfc3339(v["dateTime"].as_str()?)?;
    Some((ms, false, v["timeZone"].as_str().unwrap_or_default().to_string()))
}

fn partstat_from(s: &str) -> String {
    match s {
        "accepted" => "ACCEPTED",
        "declined" => "DECLINED",
        "tentative" => "TENTATIVE",
        _ => "NEEDS-ACTION",
    }
    .into()
}

fn partstat_to(s: &str) -> &'static str {
    match s {
        "ACCEPTED" => "accepted",
        "DECLINED" => "declined",
        "TENTATIVE" => "tentative",
        _ => "needsAction",
    }
}

pub fn event_from_google(v: &Value, calendar_id: &str, cal_tz: &str, default_reminders: &[i64]) -> Option<Event> {
    let (start, all_day, tz) = when(&v["start"])?;
    let (end, _, _) = when(&v["end"]).unwrap_or((start, all_day, String::new()));
    let mut rrule = None;
    let mut exdates = Vec::new();
    for line in v["recurrence"].as_array().into_iter().flatten().filter_map(|l| l.as_str()) {
        let Some(p) = lines::parse_line(line) else { continue };
        match p.name.as_str() {
            "RRULE" => rrule = Some(p.value.clone()),
            "EXDATE" => {
                for val in p.value.split(',') {
                    let one = lines::Prop { name: p.name.clone(), params: p.params.clone(), value: val.to_string() };
                    if let Some((ms, _, _)) = super::ical::prop_time(&one, &[]) {
                        exdates.push(ms);
                    }
                }
            }
            _ => {}
        }
    }
    let reminders = if v["reminders"]["useDefault"].as_bool().unwrap_or(false) {
        default_reminders.to_vec()
    } else {
        v["reminders"]["overrides"].as_array().into_iter().flatten().filter_map(|o| o["minutes"].as_i64()).collect()
    };
    Some(Event {
        id: String::new(),
        calendar_id: calendar_id.to_string(),
        uid: v["iCalUID"].as_str().or(v["id"].as_str()).unwrap_or_default().to_string(),
        title: v["summary"].as_str().unwrap_or_default().to_string(),
        location: v["location"].as_str().unwrap_or_default().to_string(),
        description: v["description"].as_str().unwrap_or_default().to_string(),
        start,
        end: end.max(start),
        all_day,
        timezone: if all_day { String::new() } else if tz.is_empty() { cal_tz.to_string() } else { tz },
        rrule,
        exdates,
        recurrence_id: v.get("originalStartTime").and_then(when).map(|w| w.0),
        status: v["status"].as_str().unwrap_or("confirmed").to_ascii_uppercase(),
        organizer: v["organizer"]["email"].as_str().map(|em| Organizer {
            email: em.to_ascii_lowercase(),
            name: v["organizer"]["displayName"].as_str().map(String::from),
        }),
        attendees: v["attendees"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                Some(Attendee {
                    email: a["email"].as_str()?.to_ascii_lowercase(),
                    name: a["displayName"].as_str().map(String::from),
                    partstat: partstat_from(a["responseStatus"].as_str().unwrap_or("needsAction")),
                    optional: a["optional"].as_bool().unwrap_or(false),
                })
            })
            .collect(),
        reminders,
        sequence: v["sequence"].as_i64().unwrap_or(0),
        read_only: false,
    })
}

fn gwhen(ms: i64, all_day: bool, tz: &str) -> Value {
    let d = DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_default();
    if all_day {
        return json!({ "date": d.format("%Y-%m-%d").to_string() });
    }
    let mut v = json!({ "dateTime": d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true) });
    if !tz.is_empty() {
        v["timeZone"] = json!(tz);
    }
    v
}

fn exdate_line(ms: i64, all_day: bool, tz: &str) -> String {
    let d = DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_default();
    if all_day {
        return format!("EXDATE;VALUE=DATE:{}", d.format("%Y%m%d"));
    }
    match tz.parse::<chrono_tz::Tz>() {
        Ok(z) => format!("EXDATE;TZID={}:{}", z.name(), d.with_timezone(&z).format("%Y%m%dT%H%M%S")),
        Err(_) => format!("EXDATE:{}", d.format("%Y%m%dT%H%M%SZ")),
    }
}

pub fn event_to_google(e: &Event) -> Value {
    // Google requires a time zone on recurring timed events.
    let tz = if e.rrule.is_some() && e.timezone.is_empty() && !e.all_day { "UTC".to_string() } else { e.timezone.clone() };
    let mut v = json!({
        "summary": e.title,
        "location": e.location,
        "description": e.description,
        "start": gwhen(e.start, e.all_day, &tz),
        "end": gwhen(e.end.max(e.start + if e.all_day { 86_400_000 } else { 0 }), e.all_day, &tz),
        "status": e.status.to_ascii_lowercase(),
        "reminders": { "useDefault": false, "overrides": e.reminders.iter().take(5).map(|m| json!({ "method": "popup", "minutes": m })).collect::<Vec<_>>() },
        "attendees": e.attendees.iter().map(|a| {
            let mut x = json!({ "email": a.email, "responseStatus": partstat_to(&a.partstat) });
            if let Some(n) = &a.name { x["displayName"] = json!(n); }
            if a.optional { x["optional"] = json!(true); }
            x
        }).collect::<Vec<_>>(),
    });
    if let Some(r) = &e.rrule {
        let mut rec = vec![format!("RRULE:{r}")];
        rec.extend(e.exdates.iter().map(|x| exdate_line(*x, e.all_day, &tz)));
        v["recurrence"] = json!(rec);
    }
    v
}

// ── pull ────────────────────────────────────────────────────────────────────

async fn pull_calendar(e: &Engine, token: &str, cal_id: &str, remote: &str) -> Result<()> {
    let http = &e.inner.http;
    let cid = cal_id.to_string();
    let (_, _, sync_token, _) = e.pim().store.call(move |c| store::calendar_remote(c, &cid)).await?;
    let mut st = sync_token;
    'restart: loop {
        let mut page: Option<String> = None;
        let mut items: Vec<Value> = Vec::new();
        // Assigned from every page; definitely set once the page loop breaks.
        let mut cal_tz: String;
        let mut defaults: Vec<i64>;
        let next_sync;
        loop {
            let mut u = format!("{CAL}/calendars/{}/events?maxResults=2500&showDeleted=true", enc(remote));
            if let Some(t) = &st {
                u.push_str(&format!("&syncToken={}", enc(t)));
            }
            if let Some(p) = &page {
                u.push_str(&format!("&pageToken={}", enc(p)));
            }
            match rest(http, Method::GET, &u, token, None, &[]).await {
                Ok((_, v)) => {
                    cal_tz = v["timeZone"].as_str().unwrap_or_default().to_string();
                    defaults = v["defaultReminders"].as_array().into_iter().flatten().filter_map(|r| r["minutes"].as_i64()).collect();
                    items.extend(v["items"].as_array().cloned().unwrap_or_default());
                    page = v["nextPageToken"].as_str().map(String::from);
                    if page.is_none() {
                        next_sync = v["nextSyncToken"].as_str().map(String::from);
                        break;
                    }
                }
                Err(err) if http_status(&err) == Some(410) && st.is_some() => {
                    // Token expired: wipe clean rows and do a full sync.
                    let cid = cal_id.to_string();
                    e.pim().store.call(move |c| store::clear_calendar(c, &cid)).await?;
                    st = None;
                    continue 'restart;
                }
                Err(err) => return Err(err),
            }
        }
        let cid = cal_id.to_string();
        let full = st.is_none();
        e.pim()
            .store
            .call(move |c| {
                for it in &items {
                    let gid = it["id"].as_str().unwrap_or_default().to_string();
                    let cancelled = it["status"].as_str() == Some("cancelled");
                    let series = it["recurringEventId"].as_str();
                    if cancelled && series.is_none() {
                        store::remove_events_by_remote(c, &cid, &gid)?;
                        continue;
                    }
                    // A cancelled instance carries only id/recurringEventId/originalStartTime: keep it as a
                    // CANCELLED override so that occurrence disappears.
                    let ev = if cancelled {
                        let Some(rid) = it.get("originalStartTime").and_then(when) else { continue };
                        let master = store::events_by_remote(c, &cid, series.unwrap_or_default())?.into_iter().find(|r| r.event.recurrence_id.is_none());
                        let Some(m) = master else { continue };
                        Event { recurrence_id: Some(rid.0), status: "CANCELLED".into(), start: rid.0, end: rid.0 + (m.event.end - m.event.start), id: String::new(), rrule: None, exdates: vec![], ..m.event }
                    } else {
                        match event_from_google(it, &cid, &cal_tz, &defaults) {
                            Some(ev) => ev,
                            None => continue,
                        }
                    };
                    store::upsert_remote_event(c, ev, &gid, series, it["etag"].as_str())?;
                }
                if full {
                    // Full listing: anything clean that wasn't listed is gone.
                    let listed: std::collections::HashSet<&str> = items.iter().filter_map(|i| i["id"].as_str()).collect();
                    for (rid, _) in store::remote_ids(c, &cid)? {
                        if !listed.contains(rid.as_str()) {
                            store::remove_events_by_remote(c, &cid, &rid)?;
                        }
                    }
                }
                store::set_calendar_sync(c, &cid, next_sync.as_deref(), None)
            })
            .await?;
        return Ok(());
    }
}

// ── push ────────────────────────────────────────────────────────────────────

async fn push(e: &Engine, acct: &PimAccount, token: &str) -> Result<()> {
    let http = &e.inner.http;
    let id = acct.id.clone();
    let dirty = e.pim().store.call(move |c| store::dirty_events(c, &id)).await?;
    for row in dirty {
        let cid = row.event.calendar_id.clone();
        let remote_cal = e.pim().store.call(move |c| Ok(store::calendar_remote(c, &cid)?.1)).await?;
        let base = format!("{CAL}/calendars/{}/events", enc(&remote_cal));
        let ev = row.event.clone();
        let if_match: Vec<(&str, &str)> = row.etag.as_deref().map(|t| vec![("If-Match", t)]).unwrap_or_default();
        let result: Result<Option<Value>> = if row.deleted {
            match &row.remote_id {
                Some(rid) => rest(http, Method::DELETE, &format!("{base}/{}?sendUpdates=all", enc(rid)), token, None, &if_match).await.map(|_| None),
                None => Ok(None),
            }
        } else if let Some(rid) = &row.remote_id {
            rest(http, Method::PATCH, &format!("{base}/{}?sendUpdates=all", enc(rid)), token, Some(event_to_google(&ev)), &if_match).await.map(|(_, v)| Some(v))
        } else if let (Some(occ), Some(series)) = (ev.recurrence_id, row.series_remote_id.clone()) {
            // A new override of one occurrence: find that instance's id, then patch it.
            let orig = gwhen(occ, ev.all_day, &ev.timezone);
            let os = orig["dateTime"].as_str().or(orig["date"].as_str()).unwrap_or_default().to_string();
            let (_, inst) = rest(http, Method::GET, &format!("{base}/{}/instances?originalStart={}", enc(&series), enc(&os)), token, None, &[]).await?;
            match inst["items"].as_array().and_then(|a| a.first()).and_then(|i| i["id"].as_str()) {
                Some(iid) => rest(http, Method::PATCH, &format!("{base}/{}?sendUpdates=all", enc(iid)), token, Some(event_to_google(&ev)), &[]).await.map(|(_, v)| Some(v)),
                None => Err(crate::error::ApiError::server("Google no longer has that occurrence.")),
            }
        } else {
            rest(http, Method::POST, &format!("{base}?sendUpdates=all"), token, Some(event_to_google(&ev)), &[]).await.map(|(_, v)| Some(v))
        };
        let eid = ev.id.clone();
        match result {
            Ok(Some(v)) => {
                let gid = v["id"].as_str().map(String::from);
                let uid = v["iCalUID"].as_str().map(String::from);
                let etag = v["etag"].as_str().map(String::from);
                e.pim()
                    .store
                    .call(move |c| {
                        if let Some(u) = uid {
                            // Google assigns the iCalUID on insert; keep ours in step so overrides/invites match.
                            c.execute("UPDATE events SET uid=?2 WHERE id=?1", rusqlite::params![eid, u])?;
                            let mut r = store::event(c, &eid)?;
                            r.event.uid = u;
                            store::put_event(c, &r.event, gid.as_deref().or(r.remote_id.as_deref()), r.series_remote_id.as_deref(), etag.as_deref(), false)?;
                        } else {
                            store::clear_event_dirty(c, &eid, gid.as_deref(), etag.as_deref())?;
                        }
                        Ok(())
                    })
                    .await?;
            }
            Ok(None) => {
                e.pim().store.call(move |c| store::remove_event(c, &eid)).await?;
            }
            Err(err) if matches!(http_status(&err), Some(412) | Some(404) | Some(410)) => {
                e.emit(
                    "pim://notice",
                    json!({ "message": format!("“{}” was changed or removed in Google Calendar at the same time. Google's version was kept.", ev.title) }),
                );
                e.pim().store.call(move |c| Ok(c.execute("UPDATE events SET dirty=0, deleted=0, etag=NULL WHERE id=?1", [&eid])?)).await?;
                // Force a full re-read of that calendar so the server copy comes back.
                let cid = ev.calendar_id.clone();
                e.pim().store.call(move |c| store::set_calendar_sync(c, &cid, None, None)).await?;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

// ── contacts (read-only) ────────────────────────────────────────────────────

fn label(t: Option<&str>) -> String {
    t.unwrap_or_default().to_ascii_lowercase()
}

pub fn contact_from_person(p: &Value, book_id: &str) -> Contact {
    let name = p["names"].as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null);
    let org = p["organizations"].as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null);
    let bday = p["birthdays"].as_array().and_then(|a| a.first()).map(|b| &b["date"]).and_then(|d| {
        let (m, day) = (d["month"].as_i64()?, d["day"].as_i64()?);
        Some(match d["year"].as_i64() {
            Some(y) if y > 0 => format!("{y:04}-{m:02}-{day:02}"),
            _ => format!("--{m:02}-{day:02}"),
        })
    });
    let list = |key: &str, val: &str| -> Vec<Labeled> {
        p[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| Some(Labeled { label: label(x["type"].as_str()), value: x[val].as_str()?.to_string() }))
            .collect()
    };
    let rn = p["resourceName"].as_str().unwrap_or_default();
    Contact {
        id: String::new(),
        book_id: book_id.to_string(),
        uid: rn.to_string(),
        full_name: name["displayName"].as_str().unwrap_or_default().to_string(),
        given_name: name["givenName"].as_str().unwrap_or_default().to_string(),
        family_name: name["familyName"].as_str().unwrap_or_default().to_string(),
        org: org["name"].as_str().unwrap_or_default().to_string(),
        title: org["title"].as_str().unwrap_or_default().to_string(),
        emails: list("emailAddresses", "value"),
        phones: list("phoneNumbers", "value"),
        addresses: list("addresses", "formattedValue"),
        notes: p["biographies"].as_array().and_then(|a| a.first()).and_then(|b| b["value"].as_str()).unwrap_or_default().to_string(),
        birthday: bday,
        categories: Vec::new(),
        photo: None,
        read_only: true,
    }
}

async fn pull_contacts(e: &Engine, acct: &PimAccount, token: &str) -> Result<()> {
    for (remote, name, base) in [
        (
            "connections",
            "Google Contacts",
            format!("{PEOPLE}/people/me/connections?personFields=names,emailAddresses,phoneNumbers,organizations,addresses,biographies,birthdays,metadata&pageSize=1000&requestSyncToken=true"),
        ),
        ("other", "Other contacts (Google)", format!("{PEOPLE}/otherContacts?readMask=names,emailAddresses,phoneNumbers,metadata&pageSize=1000&requestSyncToken=true")),
    ] {
        let acct_id = acct.id.clone();
        let book_id = e.pim().store.call(move |c| store::upsert_book(c, &acct_id, remote, name, true)).await?;
        let bid = book_id.clone();
        let (_, _, sync_token, _) = e.pim().store.call(move |c| store::book_remote(c, &bid)).await?;
        let mut st = sync_token;
        'restart: loop {
            let mut people: Vec<Value> = Vec::new();
            let mut page: Option<String> = None;
            let next_sync;
            loop {
                let mut u = base.clone();
                if let Some(t) = &st {
                    u.push_str(&format!("&syncToken={}", enc(t)));
                }
                if let Some(p) = &page {
                    u.push_str(&format!("&pageToken={}", enc(p)));
                }
                match rest(&e.inner.http, Method::GET, &u, token, None, &[]).await {
                    Ok((_, v)) => {
                        let key = if remote == "connections" { "connections" } else { "otherContacts" };
                        people.extend(v[key].as_array().cloned().unwrap_or_default());
                        page = v["nextPageToken"].as_str().map(String::from);
                        if page.is_none() {
                            next_sync = v["nextSyncToken"].as_str().map(String::from);
                            break;
                        }
                    }
                    Err(err) if st.is_some() && (http_status(&err) == Some(400) || http_status(&err) == Some(410)) => {
                        // EXPIRED_SYNC_TOKEN → full sync.
                        st = None;
                        continue 'restart;
                    }
                    Err(err) => return Err(err),
                }
            }
            let bid = book_id.clone();
            let full = st.is_none();
            e.pim()
                .store
                .call(move |c| {
                    for p in &people {
                        let rn = p["resourceName"].as_str().unwrap_or_default().to_string();
                        if p["metadata"]["deleted"].as_bool().unwrap_or(false) {
                            store::remove_contacts_by_remote(c, &bid, &rn)?;
                            continue;
                        }
                        store::upsert_remote_contact(c, contact_from_person(p, &bid), &rn, p["etag"].as_str())?;
                    }
                    if full {
                        let listed: std::collections::HashSet<&str> = people.iter().filter_map(|p| p["resourceName"].as_str()).collect();
                        for (rid, _) in store::contact_remote_ids(c, &bid)? {
                            if !listed.contains(rid.as_str()) {
                                store::remove_contacts_by_remote(c, &bid, &rid)?;
                            }
                        }
                    }
                    store::set_book_sync(c, &bid, next_sync.as_deref(), None)
                })
                .await?;
            break;
        }
    }
    Ok(())
}

/// Create a secondary calendar (calendars.insert) — Calendar API v3 `POST /calendars` {summary}.
pub async fn create_calendar(e: &Engine, acct: &PimAccount, name: &str) -> Result<()> {
    let token = e.pim_token(acct).await?;
    rest(&e.inner.http, Method::POST, &format!("{CAL}/calendars"), &token, Some(json!({ "summary": name })), &[]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_event_roundtrip() {
        let g = json!({
            "id": "abc", "iCalUID": "abc@google.com", "status": "confirmed", "summary": "Standup",
            "start": { "dateTime": "2026-03-02T09:00:00Z", "timeZone": "Europe/London" },
            "end": { "dateTime": "2026-03-02T09:15:00Z", "timeZone": "Europe/London" },
            "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=MO", "EXDATE;TZID=Europe/London:20260309T090000"],
            "attendees": [{ "email": "Ann@x.org", "responseStatus": "tentative", "optional": true }],
            "reminders": { "useDefault": true }
        });
        let e = event_from_google(&g, "c", "Europe/London", &[10]).unwrap();
        assert_eq!(e.uid, "abc@google.com");
        assert_eq!(e.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=MO"));
        assert_eq!(e.exdates.len(), 1);
        assert_eq!(e.attendees[0].partstat, "TENTATIVE");
        assert_eq!(e.reminders, vec![10]);
        let back = event_to_google(&e);
        assert_eq!(back["recurrence"][1], "EXDATE;TZID=Europe/London:20260309T090000");
        assert_eq!(back["start"]["timeZone"], "Europe/London");
        assert_eq!(back["attendees"][0]["responseStatus"], "tentative");
    }

    #[test]
    fn all_day_and_person() {
        let g = json!({ "id": "d", "start": { "date": "2026-12-25" }, "end": { "date": "2026-12-26" } });
        let e = event_from_google(&g, "c", "", &[]).unwrap();
        assert!(e.all_day);
        assert_eq!(event_to_google(&e)["end"]["date"], "2026-12-26");
        let p = json!({ "resourceName": "people/c1", "names": [{ "displayName": "Sarah M", "givenName": "Sarah" }],
            "emailAddresses": [{ "value": "s@x.org", "type": "work" }], "birthdays": [{ "date": { "month": 4, "day": 12 } }] });
        let c = contact_from_person(&p, "b");
        assert_eq!(c.full_name, "Sarah M");
        assert_eq!(c.birthday.as_deref(), Some("--04-12"));
        assert!(c.read_only);
    }
}
