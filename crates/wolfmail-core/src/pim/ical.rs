//! iCalendar (RFC 5545) ↔ `Event`, plus iTIP (RFC 5546) REPLY generation.
//! Sources: RFC 5545 §3.3.4 DATE / §3.3.5 DATE-TIME forms (UTC "Z", TZID-local, floating), §3.6.1 VEVENT,
//! §3.6.5 VTIMEZONE (STANDARD/DAYLIGHT observances with DTSTART+RRULE/RDATE and TZOFFSETTO), §3.6.6 VALARM
//! (TRIGGER as a DURATION relative to start, or an absolute DATE-TIME), §3.8.4.4 RECURRENCE-ID, §3.8.5.1 EXDATE,
//! §3.8.4.1 ATTENDEE (PARTSTAT, ROLE=OPT-PARTICIPANT), §3.3.6 DURATION grammar; RFC 5546 §3.2.3 REPLY (one ATTENDEE,
//! same UID/SEQUENCE/RECURRENCE-ID, DTSTAMP, ORGANIZER).
use super::lines::{self, escape, Component, Prop};
use super::model::{Attendee, Event, Organizer};
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

pub const PRODID: &str = "-//IntelligentWolf Ltd//WolfMail//EN";

// ── time zones ──────────────────────────────────────────────────────────────

/// How a TZID resolves: a real IANA zone, or the rules of an embedded VTIMEZONE.
#[derive(Clone)]
pub enum Zone {
    Iana(Tz),
    Vtz(Component),
    Local,
    Utc,
}

/// Resolve a TZID to a zone, trying (1) IANA directly, (2) the IANA tail of Mozilla/Lightning-style prefixed ids
/// ("/mozilla.org/20050126_1/Europe/London"), (3) X-LIC-LOCATION in the matching VTIMEZONE, (4) the VTIMEZONE
/// rules themselves.
pub fn resolve_tzid(tzid: &str, vtimezones: &[Component]) -> Zone {
    if let Ok(tz) = tzid.parse::<Tz>() {
        return Zone::Iana(tz);
    }
    let parts: Vec<&str> = tzid.split('/').filter(|s| !s.is_empty()).collect();
    for n in (1..=parts.len().min(3)).rev() {
        let tail = parts[parts.len() - n..].join("/");
        if let Ok(tz) = tail.parse::<Tz>() {
            return Zone::Iana(tz);
        }
    }
    if let Some(vtz) = vtimezones.iter().find(|v| v.text("TZID").as_deref() == Some(tzid)) {
        if let Some(Ok(tz)) = vtz.text("X-LIC-LOCATION").map(|l| l.parse::<Tz>()) {
            return Zone::Iana(tz);
        }
        return Zone::Vtz(vtz.clone());
    }
    Zone::Local
}

/// UTC offset (seconds) in force at local time `t` per a VTIMEZONE: the observance whose most recent onset is the
/// latest one not after `t` wins (RFC 5545 §3.6.5).
fn vtz_offset(vtz: &Component, t: NaiveDateTime) -> i64 {
    let mut best: Option<(NaiveDateTime, i64)> = None;
    for obs in vtz.children.iter().filter(|c| c.name == "STANDARD" || c.name == "DAYLIGHT") {
        let Some(to) = obs.get("TZOFFSETTO").and_then(|p| parse_utc_offset(&p.value)) else { continue };
        let Some(start) = obs.get("DTSTART").and_then(|p| parse_naive(&p.value)) else { continue };
        let mut onsets = vec![start];
        if let Some(rr) = obs.get("RRULE") {
            onsets.extend(super::recur::expand_naive(start, &rr.value, t));
        }
        for p in obs.all("RDATE") {
            for v in p.value.split(',') {
                if let Some(d) = parse_naive(v) {
                    onsets.push(d);
                }
            }
        }
        for o in onsets.into_iter().filter(|o| *o <= t) {
            if best.is_none_or(|(b, _)| o > b) {
                best = Some((o, to));
            }
        }
    }
    best.map(|(_, o)| o).unwrap_or(0)
}

fn parse_utc_offset(s: &str) -> Option<i64> {
    let s = s.trim();
    let (sign, rest) = match s.as_bytes().first()? {
        b'+' => (1, &s[1..]),
        b'-' => (-1, &s[1..]),
        _ => (1, s),
    };
    if rest.len() < 4 {
        return None;
    }
    let h: i64 = rest[0..2].parse().ok()?;
    let m: i64 = rest[2..4].parse().ok()?;
    let sec: i64 = if rest.len() >= 6 { rest[4..6].parse().ok()? } else { 0 };
    Some(sign * (h * 3600 + m * 60 + sec))
}

fn parse_naive(s: &str) -> Option<NaiveDateTime> {
    let s = s.trim().trim_end_matches('Z');
    NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%S")
        .ok()
        .or_else(|| NaiveDate::parse_from_str(s, "%Y%m%d").ok().map(|d| d.and_time(NaiveTime::MIN)))
}

/// Convert a local wall-clock time in `zone` to unix ms. Ambiguous times take the earlier instant; times in a DST
/// gap move forward by the gap (what calendar apps do).
pub fn local_to_ms(t: NaiveDateTime, zone: &Zone) -> i64 {
    match zone {
        Zone::Utc => t.and_utc().timestamp_millis(),
        Zone::Iana(tz) => match tz.from_local_datetime(&t) {
            LocalResult::Single(d) => d.timestamp_millis(),
            LocalResult::Ambiguous(a, _) => a.timestamp_millis(),
            LocalResult::None => tz
                .from_local_datetime(&(t + Duration::hours(1)))
                .earliest()
                .map(|d| d.timestamp_millis())
                .unwrap_or_else(|| t.and_utc().timestamp_millis()),
        },
        Zone::Local => match chrono::Local.from_local_datetime(&t) {
            LocalResult::Single(d) => d.timestamp_millis(),
            LocalResult::Ambiguous(a, _) => a.timestamp_millis(),
            LocalResult::None => t.and_utc().timestamp_millis(),
        },
        Zone::Vtz(v) => (t.and_utc().timestamp() - vtz_offset(v, t)) * 1000,
    }
}

/// A DATE / DATE-TIME property → (unix ms, is_date, IANA tz name or "").
pub fn prop_time(p: &Prop, vtimezones: &[Component]) -> Option<(i64, bool, String)> {
    let v = p.value.trim();
    let is_date = p.param("VALUE").is_some_and(|x| x.eq_ignore_ascii_case("DATE")) || (v.len() == 8 && !v.contains('T'));
    if is_date {
        let d = NaiveDate::parse_from_str(&v[..8.min(v.len())], "%Y%m%d").ok()?;
        return Some((d.and_time(NaiveTime::MIN).and_utc().timestamp_millis(), true, String::new()));
    }
    let naive = parse_naive(v)?;
    if v.ends_with('Z') {
        return Some((naive.and_utc().timestamp_millis(), false, String::new()));
    }
    match p.param("TZID") {
        Some(tzid) => {
            let zone = resolve_tzid(tzid, vtimezones);
            let name = match &zone {
                Zone::Iana(tz) => tz.name().to_string(),
                _ => String::new(),
            };
            Some((local_to_ms(naive, &zone), false, name))
        }
        None => Some((local_to_ms(naive, &Zone::Local), false, String::new())),
    }
}

/// Values of a multi-valued date list property (EXDATE/RDATE), each converted to unix ms.
fn prop_times(p: &Prop, vtimezones: &[Component]) -> Vec<i64> {
    p.value
        .split(',')
        .filter_map(|v| prop_time(&Prop { name: p.name.clone(), params: p.params.clone(), value: v.to_string() }, vtimezones))
        .map(|(ms, _, _)| ms)
        .collect()
}

pub fn fmt_utc(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_default().format("%Y%m%dT%H%M%SZ").to_string()
}

fn fmt_date(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_default().format("%Y%m%d").to_string()
}

/// A time property for writing: VALUE=DATE for all-day, TZID-local when the event has an IANA zone, else UTC.
fn time_prop(name: &str, ms: i64, all_day: bool, tz: &str) -> Prop {
    if all_day {
        return Prop::new(name, fmt_date(ms)).with_param("VALUE", "DATE");
    }
    if let Ok(z) = tz.parse::<Tz>() {
        let local = DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_default().with_timezone(&z).naive_local();
        return Prop::new(name, local.format("%Y%m%dT%H%M%S").to_string()).with_param("TZID", z.name());
    }
    Prop::new(name, fmt_utc(ms))
}

/// DURATION (RFC 5545 §3.3.6) → seconds. "-PT15M" → -900.
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let s = s.strip_prefix('P')?;
    let mut total = 0i64;
    let mut num = String::new();
    let mut in_time = false;
    for c in s.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            'W' | 'D' | 'H' | 'M' | 'S' => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += n * match (c, in_time) {
                    ('W', _) => 7 * 86400,
                    ('D', _) => 86400,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
            _ => return None,
        }
    }
    Some(if neg { -total } else { total })
}

fn mailto(v: &str) -> String {
    let v = v.trim();
    let v = v.strip_prefix("mailto:").or_else(|| v.strip_prefix("MAILTO:")).unwrap_or(v);
    v.to_ascii_lowercase()
}

// ── VEVENT → Event ──────────────────────────────────────────────────────────

/// Parse every VEVENT in an iCalendar object (master + overrides share a UID).
pub fn events_from_ics(ics: &str, calendar_id: &str) -> Vec<Event> {
    let mut out = Vec::new();
    for cal in lines::parse(ics).into_iter().filter(|c| c.name == "VCALENDAR") {
        let vtz: Vec<Component> = cal.children_named("VTIMEZONE").cloned().collect();
        for ve in cal.children_named("VEVENT") {
            if let Some(e) = event_from_vevent(ve, &vtz, calendar_id) {
                out.push(e);
            }
        }
    }
    out
}

pub fn event_from_vevent(ve: &Component, vtz: &[Component], calendar_id: &str) -> Option<Event> {
    let (start, all_day, tz) = prop_time(ve.get("DTSTART")?, vtz)?;
    let end = if let Some(p) = ve.get("DTEND") {
        prop_time(p, vtz).map(|t| t.0).unwrap_or(start)
    } else if let Some(d) = ve.get("DURATION").and_then(|p| parse_duration(&p.value)) {
        start + d * 1000
    } else if all_day {
        start + 86_400_000
    } else {
        start
    };
    let mut exdates = Vec::new();
    for p in ve.all("EXDATE") {
        exdates.extend(prop_times(p, vtz));
    }
    let attendees = ve
        .all("ATTENDEE")
        .map(|p| Attendee {
            email: mailto(&p.value),
            name: p.param("CN").map(|s| s.to_string()).filter(|s| !s.is_empty()),
            partstat: p.param("PARTSTAT").unwrap_or("NEEDS-ACTION").to_ascii_uppercase(),
            optional: p.param("ROLE").is_some_and(|r| r.eq_ignore_ascii_case("OPT-PARTICIPANT") || r.eq_ignore_ascii_case("NON-PARTICIPANT")),
        })
        .filter(|a| !a.email.is_empty())
        .collect();
    let organizer = ve.get("ORGANIZER").map(|p| Organizer {
        email: mailto(&p.value),
        name: p.param("CN").map(|s| s.to_string()).filter(|s| !s.is_empty()),
    });
    let reminders = ve
        .children_named("VALARM")
        .filter(|a| a.text("ACTION").is_none_or(|x| x.eq_ignore_ascii_case("DISPLAY") || x.eq_ignore_ascii_case("AUDIO") || x.eq_ignore_ascii_case("EMAIL")))
        .filter_map(|a| {
            let t = a.get("TRIGGER")?;
            if t.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME")) {
                let (abs, _, _) = prop_time(t, vtz)?;
                return Some(((start - abs) / 60_000).max(0));
            }
            let secs = parse_duration(&t.value)?;
            let related_end = t.param("RELATED").is_some_and(|r| r.eq_ignore_ascii_case("END"));
            let base = if related_end { (end - start) / 1000 } else { 0 };
            Some((-(base + secs) / 60).max(0))
        })
        .collect();
    Some(Event {
        id: String::new(),
        calendar_id: calendar_id.to_string(),
        uid: ve.text("UID").unwrap_or_default(),
        title: ve.text("SUMMARY").unwrap_or_default(),
        location: ve.text("LOCATION").unwrap_or_default(),
        description: ve.text("DESCRIPTION").unwrap_or_default(),
        start,
        end: end.max(start),
        all_day,
        timezone: tz,
        rrule: ve.get("RRULE").map(|p| p.value.trim().to_string()).filter(|s| !s.is_empty()),
        exdates,
        recurrence_id: ve.get("RECURRENCE-ID").and_then(|p| prop_time(p, vtz)).map(|t| t.0),
        status: ve.text("STATUS").map(|s| s.to_ascii_uppercase()).unwrap_or_else(|| "CONFIRMED".into()),
        organizer,
        attendees,
        reminders,
        sequence: ve.text("SEQUENCE").and_then(|s| s.parse().ok()).unwrap_or(0),
        read_only: false,
    })
}

// ── Event → VEVENT / VCALENDAR ──────────────────────────────────────────────

pub fn vevent(e: &Event, dtstamp_ms: i64) -> Component {
    let mut c = Component::new("VEVENT");
    c.push(Prop::new("UID", escape(&e.uid)));
    c.push(Prop::new("DTSTAMP", fmt_utc(dtstamp_ms)));
    if let Some(rid) = e.recurrence_id {
        c.push(time_prop("RECURRENCE-ID", rid, e.all_day, &e.timezone));
    }
    c.push(time_prop("DTSTART", e.start, e.all_day, &e.timezone));
    c.push(time_prop("DTEND", e.end.max(e.start + if e.all_day { 86_400_000 } else { 0 }), e.all_day, &e.timezone));
    c.push_text("SUMMARY", &e.title);
    c.push_text("LOCATION", &e.location);
    c.push_text("DESCRIPTION", &e.description);
    if e.status != "CONFIRMED" && !e.status.is_empty() {
        c.push(Prop::new("STATUS", e.status.clone()));
    }
    if e.sequence > 0 {
        c.push(Prop::new("SEQUENCE", e.sequence.to_string()));
    }
    if let Some(r) = &e.rrule {
        c.push(Prop::new("RRULE", r.clone()));
    }
    for x in &e.exdates {
        c.push(time_prop("EXDATE", *x, e.all_day, &e.timezone));
    }
    if let Some(o) = &e.organizer {
        let mut p = Prop::new("ORGANIZER", format!("mailto:{}", o.email));
        if let Some(n) = &o.name {
            p = p.with_param("CN", n.clone());
        }
        c.push(p);
    }
    for a in &e.attendees {
        let mut p = Prop::new("ATTENDEE", format!("mailto:{}", a.email))
            .with_param("ROLE", if a.optional { "OPT-PARTICIPANT" } else { "REQ-PARTICIPANT" })
            .with_param("PARTSTAT", a.partstat.clone());
        if a.partstat == "NEEDS-ACTION" {
            p = p.with_param("RSVP", "TRUE");
        }
        if let Some(n) = &a.name {
            p = p.with_param("CN", n.clone());
        }
        c.push(p);
    }
    for m in &e.reminders {
        let mut a = Component::new("VALARM");
        a.push(Prop::new("ACTION", "DISPLAY"));
        a.push_text("DESCRIPTION", if e.title.is_empty() { "Reminder" } else { &e.title });
        a.push(Prop::new("TRIGGER", format!("-PT{}M", m.max(&0))));
        c.children.push(a);
    }
    c
}

/// One CalDAV resource: master + its overrides, with VTIMEZONEs for every IANA zone used.
pub fn calendar_object(events: &[&Event], method: Option<&str>) -> String {
    let mut cal = Component::new("VCALENDAR");
    cal.push(Prop::new("VERSION", "2.0"));
    cal.push(Prop::new("PRODID", escape(PRODID)));
    cal.push(Prop::new("CALSCALE", "GREGORIAN"));
    if let Some(m) = method {
        cal.push(Prop::new("METHOD", m));
    }
    let mut zones: Vec<String> = Vec::new();
    for e in events {
        if !e.all_day && e.timezone.parse::<Tz>().is_ok() && !zones.contains(&e.timezone) {
            zones.push(e.timezone.clone());
        }
    }
    let now = crate::db::now_ms();
    for z in &zones {
        if let Ok(tz) = z.parse::<Tz>() {
            let year = events.iter().map(|e| year_of(e.start)).min().unwrap_or(2026);
            cal.children.push(vtimezone(tz, year));
        }
    }
    for e in events {
        cal.children.push(vevent(e, now));
    }
    lines::to_string(&cal)
}

fn year_of(ms: i64) -> i32 {
    DateTime::<Utc>::from_timestamp_millis(ms).map(|d| d.year()).unwrap_or(2026)
}

/// RFC 5545 §3.2.19 requires a VTIMEZONE for each TZID used. Built from chrono-tz by finding the offset changes in
/// `year` (hourly probe) and describing each as a yearly "nth weekday of month" rule — the form every server and
/// client understands. Zones without DST get a single STANDARD observance.
pub fn vtimezone(tz: Tz, year: i32) -> Component {
    use chrono::Offset;
    let off = |t: DateTime<Utc>| t.with_timezone(&tz).offset().fix().local_minus_utc() as i64;
    let start = Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).single().unwrap_or_default();
    let mut changes: Vec<(DateTime<Utc>, i64, i64)> = Vec::new();
    let mut prev = off(start);
    for h in 1..(24 * 366) {
        let t = start + Duration::hours(h);
        let o = off(t);
        if o != prev {
            changes.push((t, prev, o));
            prev = o;
        }
    }
    let mut v = Component::new("VTIMEZONE");
    v.push(Prop::new("TZID", tz.name()));
    v.push(Prop::new("X-LIC-LOCATION", tz.name()));
    let fmt_off = |s: i64| format!("{}{:02}{:02}", if s < 0 { '-' } else { '+' }, s.abs() / 3600, (s.abs() % 3600) / 60);
    if changes.is_empty() {
        let mut st = Component::new("STANDARD");
        st.push(Prop::new("DTSTART", "19700101T000000"));
        st.push(Prop::new("TZOFFSETFROM", fmt_off(prev)));
        st.push(Prop::new("TZOFFSETTO", fmt_off(prev)));
        v.children.push(st);
        return v;
    }
    for (at, from, to) in changes {
        // The onset is expressed in local time *before* the change (RFC 5545: DTSTART of an observance is local
        // time in the offset that was in force, i.e. TZOFFSETFROM).
        let local = (at + Duration::seconds(from)).naive_utc();
        let kind = if to > from { "DAYLIGHT" } else { "STANDARD" };
        let d = local.date();
        let dim = days_in_month(d.year(), d.month());
        let ord = if d.day() + 7 > dim { -1 } else { ((d.day() - 1) / 7 + 1) as i32 };
        let wd_i = d.weekday().num_days_from_monday();
        let wd = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"][wd_i as usize];
        // Anchor the rule in 1970 so every later date has an earlier onset to resolve against (an observance only
        // applies from its first onset — RFC 5545 §3.6.5).
        let first = nth_weekday(1970, d.month(), wd_i, ord).and_time(local.time());
        let mut ob = Component::new(kind);
        ob.push(Prop::new("DTSTART", first.format("%Y%m%dT%H%M%S").to_string()));
        ob.push(Prop::new("TZOFFSETFROM", fmt_off(from)));
        ob.push(Prop::new("TZOFFSETTO", fmt_off(to)));
        ob.push(Prop::new("RRULE", format!("FREQ=YEARLY;BYMONTH={};BYDAY={}{}", d.month(), ord, wd)));
        v.children.push(ob);
    }
    v
}

/// The `ord`-th (1-based; -1 = last) weekday `wd` (0 = Monday) of a month.
fn nth_weekday(y: i32, m: u32, wd: u32, ord: i32) -> NaiveDate {
    let fallback = NaiveDate::from_ymd_opt(y, m, 1).unwrap_or_default();
    if ord < 0 {
        let last = NaiveDate::from_ymd_opt(y, m, days_in_month(y, m)).unwrap_or(fallback);
        let back = (7 + last.weekday().num_days_from_monday() - wd) % 7;
        return last - Duration::days(back as i64);
    }
    let first = fallback;
    let fwd = (7 + wd - first.weekday().num_days_from_monday()) % 7;
    first + Duration::days(fwd as i64 + 7 * (ord as i64 - 1))
}

fn days_in_month(y: i32, m: u32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(30)
}

// ── iTIP REPLY ──────────────────────────────────────────────────────────────

/// METHOD:REPLY for one attendee (RFC 5546 §3.2.3): UID, SEQUENCE, RECURRENCE-ID and DTSTART/DTEND/SUMMARY of the
/// request, the ORGANIZER, and only the replying ATTENDEE with its new PARTSTAT.
pub fn itip_reply(req: &Event, me: &Attendee, partstat: &str) -> String {
    let mut e = req.clone();
    e.rrule = None;
    e.exdates.clear();
    e.reminders.clear();
    e.attendees = vec![Attendee { partstat: partstat.to_string(), ..me.clone() }];
    let mut cal = Component::new("VCALENDAR");
    cal.push(Prop::new("VERSION", "2.0"));
    cal.push(Prop::new("PRODID", escape(PRODID)));
    cal.push(Prop::new("METHOD", "REPLY"));
    let mut ve = vevent(&e, crate::db::now_ms());
    // REPLY attendees carry no RSVP/ROLE noise.
    for p in ve.props.iter_mut().filter(|p| p.name == "ATTENDEE") {
        p.params.retain(|(k, _)| k == "PARTSTAT" || k == "CN");
    }
    cal.children.push(ve);
    lines::to_string(&cal)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONDON: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTIMEZONE\r\nTZID:Custom London\r\nBEGIN:DAYLIGHT\r\nTZOFFSETFROM:+0000\r\nTZOFFSETTO:+0100\r\nDTSTART:19810329T010000\r\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU\r\nEND:DAYLIGHT\r\nBEGIN:STANDARD\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0000\r\nDTSTART:19961027T020000\r\nRRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:a1\r\nDTSTART;TZID=Custom London:20260715T090000\r\nDTEND;TZID=Custom London:20260715T100000\r\nSUMMARY:Summer\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn vtimezone_rules_are_evaluated() {
        let ev = &events_from_ics(LONDON, "c")[0];
        // 09:00 BST = 08:00 UTC
        assert_eq!(fmt_utc(ev.start), "20260715T080000Z");
        assert_eq!(ev.reminders, vec![15]);
    }

    #[test]
    fn prefixed_tzid_and_all_day_and_duration() {
        let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:b\nDTSTART;TZID=/mozilla.org/20050126_1/Europe/London:20260115T090000\nDURATION:PT1H30M\nEND:VEVENT\nBEGIN:VEVENT\nUID:c\nDTSTART;VALUE=DATE:20261225\nEND:VEVENT\nEND:VCALENDAR\n";
        let evs = events_from_ics(ics, "c");
        assert_eq!(evs[0].timezone, "Europe/London");
        assert_eq!(fmt_utc(evs[0].start), "20260115T090000Z");
        assert_eq!(evs[0].end - evs[0].start, 90 * 60_000);
        assert!(evs[1].all_day);
        assert_eq!(evs[1].end - evs[1].start, 86_400_000);
    }

    #[test]
    fn roundtrip_with_generated_vtimezone() {
        let e = Event {
            calendar_id: "c".into(),
            uid: "u1".into(),
            title: "Weekly, sync; team".into(),
            start: Utc.with_ymd_and_hms(2026, 3, 2, 9, 0, 0).unwrap().timestamp_millis(),
            end: Utc.with_ymd_and_hms(2026, 3, 2, 10, 0, 0).unwrap().timestamp_millis(),
            timezone: "Europe/London".into(),
            rrule: Some("FREQ=WEEKLY;BYDAY=MO".into()),
            exdates: vec![Utc.with_ymd_and_hms(2026, 3, 9, 9, 0, 0).unwrap().timestamp_millis()],
            attendees: vec![Attendee { email: "a@x.org".into(), name: Some("A, B".into()), partstat: "NEEDS-ACTION".into(), optional: true }],
            reminders: vec![10],
            status: "CONFIRMED".into(),
            ..Default::default()
        };
        let ics = calendar_object(&[&e], None);
        assert!(ics.contains("BEGIN:VTIMEZONE"));
        assert!(ics.contains("TZID=Europe/London"));
        let back = &events_from_ics(&ics, "c")[0];
        assert_eq!(back.start, e.start);
        assert_eq!(back.title, e.title);
        assert_eq!(back.rrule, e.rrule);
        assert_eq!(back.exdates, e.exdates);
        assert_eq!(back.attendees, e.attendees);
        assert_eq!(back.reminders, vec![10]);
    }

    #[test]
    fn generated_vtimezone_matches_chrono_tz_offsets() {
        let tz: Tz = "America/New_York".parse().unwrap();
        let v = vtimezone(tz, 2026);
        // A summer and a winter instant convert the same through the VTIMEZONE as through chrono-tz.
        for (y, m, d, h) in [(2026, 7, 4, 12), (2026, 1, 10, 12), (2027, 7, 4, 12)] {
            let naive = NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, 0, 0).unwrap();
            assert_eq!(local_to_ms(naive, &Zone::Vtz(v.clone())), local_to_ms(naive, &Zone::Iana(tz)));
        }
    }

    #[test]
    fn reply_has_one_attendee_and_method() {
        let req = &events_from_ics("BEGIN:VCALENDAR\nMETHOD:REQUEST\nBEGIN:VEVENT\nUID:m1\nSEQUENCE:2\nDTSTART:20261101T100000Z\nDTEND:20261101T110000Z\nSUMMARY:Call\nORGANIZER;CN=Sarah:mailto:sarah@x.org\nATTENDEE;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:me@y.org\nATTENDEE:mailto:other@z.org\nEND:VEVENT\nEND:VCALENDAR\n", "c")[0];
        let me = req.attendees[0].clone();
        let r = itip_reply(req, &me, "ACCEPTED");
        assert!(r.contains("METHOD:REPLY"));
        assert!(r.contains("SEQUENCE:2"));
        assert!(r.contains("PARTSTAT=ACCEPTED"));
        assert!(!r.contains("other@z.org"));
        assert!(r.contains("ORGANIZER;CN=Sarah:mailto:sarah@x.org"));
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("-PT15M"), Some(-900));
        assert_eq!(parse_duration("P1W"), Some(604_800));
        assert_eq!(parse_duration("-P1DT2H"), Some(-(86400 + 7200)));
        assert_eq!(parse_duration("PT0S"), Some(0));
        assert_eq!(parse_duration("garbage"), None);
    }
}
