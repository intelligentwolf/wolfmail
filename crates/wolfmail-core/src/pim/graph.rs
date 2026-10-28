//! Microsoft 365 / Outlook.com: Microsoft Graph calendars, events and contacts.
//! Sources (Microsoft Learn, fetched 2026-10-28):
//! - calendar resource: id, name, hexColor, canEdit, isDefaultCalendar; `GET /me/calendars`.
//! - List calendarView: `GET /me/calendars/{id}/calendarView?startDateTime=&endDateTime=` (ISO 8601; no offset = UTC),
//!   expands occurrences/exceptions/single instances; `Prefer: outlook.timezone` (default UTC); `$top` ≤ 1000;
//!   `@odata.nextLink` paging. Permission Calendars.ReadBasic+ (we hold Calendars.ReadWrite).
//! - event resource: subject, body{contentType,content}, start/end dateTimeTimeZone, isAllDay, location.displayName,
//!   attendees[{emailAddress{name,address}, type, status{response}}], organizer, isReminderOn,
//!   reminderMinutesBeforeStart, type singleInstance|occurrence|exception|seriesMaster, seriesMasterId, iCalUId
//!   (different for each occurrence), changeKey, isCancelled.
//! - responseStatus.response: none|organizer|tentativelyAccepted|accepted|declined|notResponded.
//! - patternedRecurrence: pattern{type daily|weekly|absoluteMonthly|relativeMonthly|absoluteYearly|relativeYearly,
//!   interval, daysOfWeek, firstDayOfWeek, dayOfMonth, month, index first|second|third|fourth|last} and
//!   range{type endDate|noEnd|numbered, startDate, endDate, numberOfOccurrences}.
//! - dateTimeTimeZone.timeZone: Windows names "as well as" the listed additional IANA zones (list below).
//! - contact resource: displayName, givenName, surname, companyName, jobTitle, emailAddresses[{name,address}],
//!   businessPhones[], homePhones[], mobilePhone, home/business/otherAddress physicalAddress, personalNotes,
//!   birthday (DateTimeOffset), categories, changeKey; `GET/POST /me/contacts`, `PATCH/DELETE /me/contacts/{id}`.
use super::model::*;
use super::{rest, store};
use crate::engine::Engine;
use crate::error::{ApiError, Result};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, Utc};
use reqwest::Method;
use serde_json::{json, Value};

const G: &str = "https://graph.microsoft.com/v1.0";
/// Window of occurrences kept locally (calendarView needs a range).
const PAST_DAYS: i64 = 180;
const FUTURE_DAYS: i64 = 540;

/// Source: Microsoft Learn "dateTimeTimeZone resource type", "Additional time zones" list (2025-02-11 revision).
const GRAPH_IANA: &[&str] = &[
    "Etc/GMT+12", "Etc/GMT+11", "Pacific/Honolulu", "America/Anchorage", "America/Santa_Isabel", "America/Los_Angeles",
    "America/Phoenix", "America/Chihuahua", "America/Denver", "America/Guatemala", "America/Chicago", "America/Mexico_City",
    "America/Regina", "America/Bogota", "America/New_York", "America/Indiana/Indianapolis", "America/Caracas", "America/Asuncion",
    "America/Halifax", "America/Cuiaba", "America/La_Paz", "America/Santiago", "America/St_Johns", "America/Sao_Paulo",
    "America/Argentina/Buenos_Aires", "America/Cayenne", "America/Godthab", "America/Montevideo", "America/Bahia", "Etc/GMT+2",
    "Atlantic/Azores", "Atlantic/Cape_Verde", "Africa/Casablanca", "Etc/GMT", "Europe/London", "Atlantic/Reykjavik",
    "Europe/Berlin", "Europe/Budapest", "Europe/Paris", "Europe/Warsaw", "Africa/Lagos", "Africa/Windhoek", "Europe/Bucharest",
    "Asia/Beirut", "Africa/Cairo", "Asia/Damascus", "Africa/Johannesburg", "Europe/Kyiv", "Europe/Istanbul", "Asia/Jerusalem",
    "Asia/Amman", "Asia/Baghdad", "Europe/Kaliningrad", "Asia/Riyadh", "Africa/Nairobi", "Asia/Tehran", "Asia/Dubai", "Asia/Baku",
    "Europe/Moscow", "Indian/Mauritius", "Asia/Tbilisi", "Asia/Yerevan", "Asia/Kabul", "Asia/Karachi", "Asia/Tashkent",
    "Asia/Kolkata", "Asia/Colombo", "Asia/Kathmandu", "Asia/Almaty", "Asia/Dhaka", "Asia/Yekaterinburg", "Asia/Rangoon",
    "Asia/Bangkok", "Asia/Novosibirsk", "Asia/Shanghai", "Asia/Krasnoyarsk", "Asia/Singapore", "Australia/Perth", "Asia/Taipei",
    "Asia/Ulaanbaatar", "Asia/Irkutsk", "Asia/Tokyo", "Asia/Seoul", "Australia/Adelaide", "Australia/Darwin", "Australia/Brisbane",
    "Australia/Sydney", "Pacific/Port_Moresby", "Australia/Hobart", "Asia/Yakutsk", "Pacific/Guadalcanal", "Asia/Vladivostok",
    "Pacific/Auckland", "Etc/GMT-12", "Pacific/Fiji", "Asia/Magadan", "Pacific/Tongatapu", "Pacific/Apia", "Pacific/Kiritimati",
];

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn parse_graph_dt(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim().trim_end_matches('Z');
    let main = s.split('.').next().unwrap_or(s);
    NaiveDateTime::parse_from_str(main, "%Y-%m-%dT%H:%M:%S").ok()
}

pub fn event_from_graph(v: &Value, calendar_id: &str) -> Option<Event> {
    // Times come back in UTC (no Prefer header).
    let s = parse_graph_dt(v["start"]["dateTime"].as_str()?)?;
    let en = parse_graph_dt(v["end"]["dateTime"].as_str()?).unwrap_or(s);
    let all_day = v["isAllDay"].as_bool().unwrap_or(false);
    let (start, end) = if all_day {
        // All-day items are midnight-to-midnight in the event's own zone; in UTC they can sit up to ±14h off
        // midnight, so round to the nearest date.
        let round = |t: NaiveDateTime| {
            let d = if t.time() >= chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap_or_default() { t.date() + Duration::days(1) } else { t.date() };
            d.and_hms_opt(0, 0, 0).unwrap_or_default().and_utc().timestamp_millis()
        };
        (round(s), round(en))
    } else {
        (s.and_utc().timestamp_millis(), en.and_utc().timestamp_millis())
    };
    let body = v["body"]["content"].as_str().unwrap_or_default();
    let description = if v["body"]["contentType"].as_str() == Some("html") { crate::mime::html_to_text(body) } else { body.to_string() };
    let tz = v["originalStartTimeZone"].as_str().filter(|z| z.parse::<chrono_tz::Tz>().is_ok()).unwrap_or_default().to_string();
    Some(Event {
        id: String::new(),
        calendar_id: calendar_id.to_string(),
        uid: v["iCalUId"].as_str().or(v["id"].as_str()).unwrap_or_default().to_string(),
        title: v["subject"].as_str().unwrap_or_default().to_string(),
        location: v["location"]["displayName"].as_str().unwrap_or_default().to_string(),
        description: description.trim().to_string(),
        start,
        end: end.max(start),
        all_day,
        timezone: if all_day { String::new() } else { tz },
        rrule: None,
        exdates: vec![],
        recurrence_id: None,
        status: if v["isCancelled"].as_bool().unwrap_or(false) {
            "CANCELLED".into()
        } else if v["showAs"].as_str() == Some("tentative") {
            "TENTATIVE".into()
        } else {
            "CONFIRMED".into()
        },
        organizer: v["organizer"]["emailAddress"]["address"].as_str().map(|a| Organizer {
            email: a.to_ascii_lowercase(),
            name: v["organizer"]["emailAddress"]["name"].as_str().map(String::from),
        }),
        attendees: v["attendees"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                Some(Attendee {
                    email: a["emailAddress"]["address"].as_str()?.to_ascii_lowercase(),
                    name: a["emailAddress"]["name"].as_str().map(String::from),
                    partstat: match a["status"]["response"].as_str().unwrap_or("none") {
                        "accepted" | "organizer" => "ACCEPTED",
                        "declined" => "DECLINED",
                        "tentativelyAccepted" => "TENTATIVE",
                        _ => "NEEDS-ACTION",
                    }
                    .into(),
                    optional: a["type"].as_str() == Some("optional"),
                })
            })
            .collect(),
        reminders: if v["isReminderOn"].as_bool().unwrap_or(false) { v["reminderMinutesBeforeStart"].as_i64().into_iter().collect() } else { vec![] },
        sequence: 0,
        read_only: false,
    })
}

fn graph_when(ms: i64, all_day: bool, tz: &str) -> Value {
    let utc = DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_default();
    if all_day {
        return json!({ "dateTime": utc.format("%Y-%m-%dT00:00:00").to_string(), "timeZone": "UTC" });
    }
    if GRAPH_IANA.contains(&tz) {
        if let Ok(z) = tz.parse::<chrono_tz::Tz>() {
            return json!({ "dateTime": utc.with_timezone(&z).format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": tz });
        }
    }
    json!({ "dateTime": utc.format("%Y-%m-%dT%H:%M:%S").to_string(), "timeZone": "UTC" })
}

const DAYS: [(&str, &str); 7] =
    [("MO", "monday"), ("TU", "tuesday"), ("WE", "wednesday"), ("TH", "thursday"), ("FR", "friday"), ("SA", "saturday"), ("SU", "sunday")];

fn day_name(code: &str) -> Option<&'static str> {
    DAYS.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

/// RRULE → patternedRecurrence for the subset Graph can express; anything else is a visible error.
pub fn rrule_to_graph(rule: &str, start_ms: i64, tz: &str) -> Result<Value> {
    let parts: std::collections::HashMap<String, String> = rule
        .split(';')
        .filter_map(|p| p.split_once('=').map(|(k, v)| (k.to_ascii_uppercase(), v.to_ascii_uppercase())))
        .collect();
    let unsupported = || ApiError::invalid("rrule", "This repeat pattern can't be saved to a Microsoft calendar. Choose daily, weekly, monthly or yearly.");
    if parts.keys().any(|k| !matches!(k.as_str(), "FREQ" | "INTERVAL" | "BYDAY" | "BYMONTHDAY" | "BYMONTH" | "COUNT" | "UNTIL" | "WKST")) {
        return Err(unsupported());
    }
    let local = {
        let utc = DateTime::<Utc>::from_timestamp_millis(start_ms).unwrap_or_default();
        match tz.parse::<chrono_tz::Tz>() {
            Ok(z) => utc.with_timezone(&z).date_naive(),
            Err(_) => utc.date_naive(),
        }
    };
    let interval: i64 = parts.get("INTERVAL").and_then(|v| v.parse().ok()).unwrap_or(1);
    let byday: Vec<(Option<i64>, &'static str)> = parts
        .get("BYDAY")
        .map(|v| {
            v.split(',')
                .filter_map(|d| {
                    let code = &d[d.len().saturating_sub(2)..];
                    let ord = d[..d.len().saturating_sub(2)].parse::<i64>().ok();
                    day_name(code).map(|n| (ord, n))
                })
                .collect()
        })
        .unwrap_or_default();
    let index = |o: i64| match o {
        1 => Ok("first"),
        2 => Ok("second"),
        3 => Ok("third"),
        4 => Ok("fourth"),
        -1 => Ok("last"),
        _ => Err(unsupported()),
    };
    let wkst = parts.get("WKST").and_then(|w| day_name(w)).unwrap_or("monday");
    let start_day = DAYS[local.weekday().num_days_from_monday() as usize].1;
    let pattern = match parts.get("FREQ").map(String::as_str) {
        Some("DAILY") => json!({ "type": "daily", "interval": interval }),
        Some("WEEKLY") => {
            let days: Vec<&str> = if byday.is_empty() { vec![start_day] } else { byday.iter().map(|(_, d)| *d).collect() };
            json!({ "type": "weekly", "interval": interval, "daysOfWeek": days, "firstDayOfWeek": wkst })
        }
        Some("MONTHLY") => match (parts.get("BYMONTHDAY"), byday.as_slice()) {
            (Some(d), []) => json!({ "type": "absoluteMonthly", "interval": interval, "dayOfMonth": d.parse::<i64>().map_err(|_| unsupported())? }),
            (None, [(Some(o), d)]) => json!({ "type": "relativeMonthly", "interval": interval, "daysOfWeek": [d], "index": index(*o)? }),
            (None, []) => json!({ "type": "absoluteMonthly", "interval": interval, "dayOfMonth": local.day() }),
            _ => return Err(unsupported()),
        },
        Some("YEARLY") => {
            let month: i64 = parts.get("BYMONTH").and_then(|m| m.parse().ok()).unwrap_or(local.month() as i64);
            match (parts.get("BYMONTHDAY"), byday.as_slice()) {
                (Some(d), []) => json!({ "type": "absoluteYearly", "interval": interval, "month": month, "dayOfMonth": d.parse::<i64>().map_err(|_| unsupported())? }),
                (None, [(Some(o), d)]) => json!({ "type": "relativeYearly", "interval": interval, "month": month, "daysOfWeek": [d], "index": index(*o)? }),
                (None, []) => json!({ "type": "absoluteYearly", "interval": interval, "month": month, "dayOfMonth": local.day() }),
                _ => return Err(unsupported()),
            }
        }
        _ => return Err(unsupported()),
    };
    let start_date = local.format("%Y-%m-%d").to_string();
    let range = if let Some(c) = parts.get("COUNT") {
        json!({ "type": "numbered", "startDate": start_date, "numberOfOccurrences": c.parse::<i64>().map_err(|_| unsupported())? })
    } else if let Some(u) = parts.get("UNTIL") {
        let d = NaiveDate::parse_from_str(&u[..8.min(u.len())], "%Y%m%d").map_err(|_| unsupported())?;
        json!({ "type": "endDate", "startDate": start_date, "endDate": d.format("%Y-%m-%d").to_string() })
    } else {
        json!({ "type": "noEnd", "startDate": start_date })
    };
    Ok(json!({ "pattern": pattern, "range": range }))
}

pub fn event_to_graph(e: &Event, with_recurrence: bool) -> Result<Value> {
    let mut v = json!({
        "subject": e.title,
        "body": { "contentType": "text", "content": e.description },
        "start": graph_when(e.start, e.all_day, &e.timezone),
        "end": graph_when(e.end.max(e.start + if e.all_day { 86_400_000 } else { 0 }), e.all_day, &e.timezone),
        "isAllDay": e.all_day,
        "location": { "displayName": e.location },
        "isReminderOn": !e.reminders.is_empty(),
        "attendees": e.attendees.iter().map(|a| json!({
            "emailAddress": { "address": a.email, "name": a.name.clone().unwrap_or_default() },
            "type": if a.optional { "optional" } else { "required" },
        })).collect::<Vec<_>>(),
    });
    if let Some(m) = e.reminders.first() {
        v["reminderMinutesBeforeStart"] = json!(m);
    }
    if with_recurrence {
        if let Some(r) = &e.rrule {
            v["recurrence"] = rrule_to_graph(r, e.start, &e.timezone)?;
        }
    }
    Ok(v)
}

async fn get_all(e: &Engine, token: &str, first: &str) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    let mut url = Some(first.to_string());
    while let Some(u) = url {
        let (_, v) = rest(&e.inner.http, Method::GET, &u, token, None, &[]).await?;
        out.extend(v["value"].as_array().cloned().unwrap_or_default());
        url = v["@odata.nextLink"].as_str().map(String::from);
    }
    Ok(out)
}

pub async fn sync(e: &Engine, acct: &PimAccount) -> Result<()> {
    let token = e.pim_token(acct).await?;
    let cals = get_all(e, &token, &format!("{G}/me/calendars?$top=100")).await?;
    let acct_id = acct.id.clone();
    let regs = e
        .pim()
        .store
        .call(move |c| {
            let mut out = Vec::new();
            let mut keep = Vec::new();
            for cal in &cals {
                let rid = cal["id"].as_str().unwrap_or_default().to_string();
                let color = cal["hexColor"].as_str().filter(|h| h.len() == 7).unwrap_or("#2563eb").to_string();
                let id = store::upsert_calendar(
                    c,
                    &acct_id,
                    &rid,
                    cal["name"].as_str().unwrap_or("Calendar"),
                    &color,
                    !cal["canEdit"].as_bool().unwrap_or(true),
                    cal["isDefaultCalendar"].as_bool().unwrap_or(false),
                )?;
                keep.push(rid.clone());
                out.push((id, rid));
            }
            store::prune_calendars(c, &acct_id, &keep)?;
            Ok(out)
        })
        .await?;
    let now = Utc::now();
    let from = (now - Duration::days(PAST_DAYS)).format("%Y-%m-%dT00:00:00").to_string();
    let to = (now + Duration::days(FUTURE_DAYS)).format("%Y-%m-%dT00:00:00").to_string();
    for (cal_id, rid) in regs {
        let items = get_all(e, &token, &format!("{G}/me/calendars/{}/calendarView?startDateTime={from}&endDateTime={to}&$top=500", enc(&rid))).await?;
        let cid = cal_id.clone();
        e.pim()
            .store
            .call(move |c| {
                let mut seen = std::collections::HashSet::new();
                for it in &items {
                    let Some(ev) = event_from_graph(it, &cid) else { continue };
                    let gid = it["id"].as_str().unwrap_or_default().to_string();
                    seen.insert(gid.clone());
                    store::upsert_remote_event(c, ev, &gid, it["seriesMasterId"].as_str(), it["changeKey"].as_str())?;
                }
                for (rid, _) in store::remote_ids(c, &cid)? {
                    if !seen.contains(&rid) {
                        store::remove_events_by_remote(c, &cid, &rid)?;
                    }
                }
                Ok(())
            })
            .await?;
    }
    // Contacts
    let people = get_all(e, &token, &format!("{G}/me/contacts?$top=500")).await?;
    let acct_id = acct.id.clone();
    e.pim()
        .store
        .call(move |c| {
            let bid = store::upsert_book(c, &acct_id, "contacts", "Outlook contacts", false)?;
            let mut seen = std::collections::HashSet::new();
            for p in &people {
                let gid = p["id"].as_str().unwrap_or_default().to_string();
                seen.insert(gid.clone());
                store::upsert_remote_contact(c, contact_from_graph(p, &bid), &gid, p["changeKey"].as_str())?;
            }
            for (rid, _) in store::contact_remote_ids(c, &bid)? {
                if !seen.contains(&rid) {
                    store::remove_contacts_by_remote(c, &bid, &rid)?;
                }
            }
            Ok(())
        })
        .await?;
    Ok(())
}

// ── writes (online: Graph series edits need the server, so they are not queued) ──

pub async fn create_event(e: &Engine, acct: &PimAccount, cal_remote: &str, ev: &Event) -> Result<()> {
    let token = e.pim_token(acct).await?;
    rest(&e.inner.http, Method::POST, &format!("{G}/me/calendars/{}/events", enc(cal_remote)), &token, Some(event_to_graph(ev, true)?), &[]).await?;
    Ok(())
}

/// `target` = the occurrence/single id (scope This) or the series master id (scope All). For a series, the time
/// change is applied as a shift of the master's start/end, so the pattern keeps its shape.
pub async fn update_event(e: &Engine, acct: &PimAccount, target: &str, ev: &Event, series_shift_ms: Option<i64>) -> Result<()> {
    let token = e.pim_token(acct).await?;
    let mut body = event_to_graph(ev, false)?;
    if let Some(shift) = series_shift_ms {
        let (_, master) = rest(&e.inner.http, Method::GET, &format!("{G}/me/events/{}", enc(target)), &token, None, &[]).await?;
        let m = event_from_graph(&master, &ev.calendar_id).ok_or_else(|| ApiError::server("Microsoft returned a series WolfMail could not read."))?;
        body["start"] = graph_when(m.start + shift, ev.all_day, &ev.timezone);
        body["end"] = graph_when(m.start + shift + (ev.end - ev.start), ev.all_day, &ev.timezone);
        if let Some(r) = &ev.rrule {
            body["recurrence"] = rrule_to_graph(r, m.start + shift, &ev.timezone)?;
        }
    }
    rest(&e.inner.http, Method::PATCH, &format!("{G}/me/events/{}", enc(target)), &token, Some(body), &[]).await?;
    Ok(())
}

pub async fn delete_event(e: &Engine, acct: &PimAccount, target: &str) -> Result<()> {
    let token = e.pim_token(acct).await?;
    match rest(&e.inner.http, Method::DELETE, &format!("{G}/me/events/{}", enc(target)), &token, None, &[]).await {
        Ok(_) => Ok(()),
        Err(err) if super::http_status(&err) == Some(404) => Ok(()),
        Err(err) => Err(err),
    }
}

/// Graph `accept` / `tentativelyAccept` / `decline` on an event in the user's calendar (sends the response).
pub async fn respond(e: &Engine, acct: &PimAccount, event_id: &str, partstat: &str) -> Result<()> {
    let token = e.pim_token(acct).await?;
    let action = match partstat {
        "ACCEPTED" => "accept",
        "TENTATIVE" => "tentativelyAccept",
        _ => "decline",
    };
    rest(&e.inner.http, Method::POST, &format!("{G}/me/events/{}/{action}", enc(event_id)), &token, Some(json!({ "sendResponse": true })), &[]).await?;
    Ok(())
}

// ── contacts ────────────────────────────────────────────────────────────────

fn addr_line(a: &Value) -> String {
    ["street", "city", "state", "postalCode", "countryOrRegion"]
        .iter()
        .filter_map(|k| a[*k].as_str())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn contact_from_graph(p: &Value, book_id: &str) -> Contact {
    let mut phones: Vec<Labeled> = Vec::new();
    if let Some(m) = p["mobilePhone"].as_str().filter(|s| !s.is_empty()) {
        phones.push(Labeled { label: "mobile".into(), value: m.into() });
    }
    for (k, l) in [("businessPhones", "work"), ("homePhones", "home")] {
        for v in p[k].as_array().into_iter().flatten().filter_map(|x| x.as_str()) {
            phones.push(Labeled { label: l.into(), value: v.into() });
        }
    }
    let addresses = [("homeAddress", "home"), ("businessAddress", "work"), ("otherAddress", "other")]
        .iter()
        .map(|(k, l)| Labeled { label: (*l).into(), value: addr_line(&p[*k]) })
        .filter(|a| !a.value.is_empty())
        .collect();
    Contact {
        id: String::new(),
        book_id: book_id.to_string(),
        uid: p["id"].as_str().unwrap_or_default().to_string(),
        full_name: p["displayName"].as_str().unwrap_or_default().to_string(),
        given_name: p["givenName"].as_str().unwrap_or_default().to_string(),
        family_name: p["surname"].as_str().unwrap_or_default().to_string(),
        org: p["companyName"].as_str().unwrap_or_default().to_string(),
        title: p["jobTitle"].as_str().unwrap_or_default().to_string(),
        emails: p["emailAddresses"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| Some(Labeled { label: String::new(), value: x["address"].as_str()?.to_string() }))
            .collect(),
        phones,
        addresses,
        notes: p["personalNotes"].as_str().unwrap_or_default().to_string(),
        birthday: p["birthday"].as_str().and_then(|b| b.get(0..10)).map(String::from),
        categories: p["categories"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect(),
        photo: None,
        read_only: false,
    }
}

pub fn contact_to_graph(c: &Contact) -> Value {
    let by = |l: &str| c.phones.iter().filter(|p| p.label == l).map(|p| p.value.clone()).collect::<Vec<_>>();
    let mut v = json!({
        "displayName": c.full_name,
        "givenName": c.given_name,
        "surname": c.family_name,
        "companyName": c.org,
        "jobTitle": c.title,
        "emailAddresses": c.emails.iter().map(|e| json!({ "address": e.value, "name": c.full_name })).collect::<Vec<_>>(),
        "businessPhones": by("work"),
        "homePhones": c.phones.iter().filter(|p| p.label != "work" && p.label != "mobile").map(|p| p.value.clone()).collect::<Vec<_>>(),
        "mobilePhone": by("mobile").into_iter().next().unwrap_or_default(),
        "personalNotes": c.notes,
        "categories": c.categories,
    });
    for (label, key) in [("home", "homeAddress"), ("work", "businessAddress"), ("other", "otherAddress")] {
        if let Some(a) = c.addresses.iter().find(|a| a.label == label) {
            v[key] = json!({ "street": a.value });
        }
    }
    if let Some(b) = c.birthday.as_deref().filter(|b| !b.starts_with("--")) {
        v["birthday"] = json!(format!("{b}T00:00:00Z"));
    }
    v
}

pub async fn save_contact(e: &Engine, acct: &PimAccount, remote_id: Option<&str>, c: &Contact) -> Result<()> {
    let token = e.pim_token(acct).await?;
    match remote_id {
        Some(id) => rest(&e.inner.http, Method::PATCH, &format!("{G}/me/contacts/{}", enc(id)), &token, Some(contact_to_graph(c)), &[]).await?,
        None => rest(&e.inner.http, Method::POST, &format!("{G}/me/contacts"), &token, Some(contact_to_graph(c)), &[]).await?,
    };
    Ok(())
}

pub async fn delete_contact(e: &Engine, acct: &PimAccount, remote_id: &str) -> Result<()> {
    let token = e.pim_token(acct).await?;
    match rest(&e.inner.http, Method::DELETE, &format!("{G}/me/contacts/{}", enc(remote_id)), &token, None, &[]).await {
        Ok(_) => Ok(()),
        Err(err) if super::http_status(&err) == Some(404) => Ok(()),
        Err(err) => Err(err),
    }
}

pub async fn create_calendar(e: &Engine, acct: &PimAccount, name: &str) -> Result<()> {
    let token = e.pim_token(acct).await?;
    rest(&e.inner.http, Method::POST, &format!("{G}/me/calendars"), &token, Some(json!({ "name": name })), &[]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrule_mapping() {
        let start = Utc.with_ymd_and_hms(2026, 3, 12, 9, 0, 0).unwrap().timestamp_millis(); // Thursday
        let w = rrule_to_graph("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,TH;COUNT=6", start, "Europe/London").unwrap();
        assert_eq!(w["pattern"]["type"], "weekly");
        assert_eq!(w["pattern"]["daysOfWeek"], json!(["monday", "thursday"]));
        assert_eq!(w["range"]["numberOfOccurrences"], 6);
        let m = rrule_to_graph("FREQ=MONTHLY;BYDAY=-1FR", start, "").unwrap();
        assert_eq!(m["pattern"]["index"], "last");
        let y = rrule_to_graph("FREQ=YEARLY;UNTIL=20300101T000000Z", start, "").unwrap();
        assert_eq!(y["pattern"]["month"], 3);
        assert_eq!(y["range"]["endDate"], "2030-01-01");
        assert!(rrule_to_graph("FREQ=MONTHLY;BYSETPOS=2;BYDAY=MO,TU", start, "").is_err());
    }

    use chrono::TimeZone;

    #[test]
    fn graph_event_and_contact() {
        let g = json!({ "id": "AAM", "iCalUId": "040000", "subject": "Review", "isAllDay": false,
            "start": { "dateTime": "2026-05-01T08:00:00.0000000", "timeZone": "UTC" },
            "end": { "dateTime": "2026-05-01T09:00:00.0000000", "timeZone": "UTC" },
            "attendees": [{ "emailAddress": { "address": "A@x.org", "name": "A" }, "type": "optional", "status": { "response": "tentativelyAccepted" } }],
            "isReminderOn": true, "reminderMinutesBeforeStart": 15, "originalStartTimeZone": "Europe/London" });
        let e = event_from_graph(&g, "c").unwrap();
        assert_eq!(e.reminders, vec![15]);
        assert_eq!(e.attendees[0].partstat, "TENTATIVE");
        let back = event_to_graph(&e, false).unwrap();
        assert_eq!(back["start"]["timeZone"], "Europe/London");
        assert_eq!(back["start"]["dateTime"], "2026-05-01T09:00:00");
        let allday = json!({ "subject": "Holiday", "isAllDay": true, "start": { "dateTime": "2026-07-31T23:00:00.0000000" }, "end": { "dateTime": "2026-08-01T23:00:00.0000000" } });
        let a = event_from_graph(&allday, "c").unwrap();
        assert_eq!(a.start, Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap().timestamp_millis());
        let p = json!({ "id": "C1", "displayName": "Tom", "mobilePhone": "07700", "businessPhones": ["0113"], "emailAddresses": [{ "address": "t@x.org" }] });
        let c = contact_from_graph(&p, "b");
        assert_eq!(c.phones.len(), 2);
        assert_eq!(contact_to_graph(&c)["mobilePhone"], "07700");
    }
}
