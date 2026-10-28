//! Recurrence expansion (RFC 5545 §3.3.10 RECUR, §3.8.5 EXDATE) through the `rrule` crate.
//! Source: rrule 0.14 src/core/rruleset.rs (`RRule::from_str` → `build(dt_start)` → `after`/`before` → `all(limit)`)
//! and src/validator/validate_rrule.rs:50-87 — UNTIL must be UTC when DTSTART has a named zone (Local allows Local
//! or UTC). Real-world files often violate that (date-only or local UNTIL), so UNTIL is normalised to UTC first,
//! interpreting a local/date-only value in the event's own zone as RFC 5545 intends (date-only = whole day).
use super::ical::{local_to_ms, Zone};
use super::model::Event;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use rrule::{RRule, Tz as RTz, Unvalidated};

/// Hard cap per expansion so a malformed "every second forever" rule can't hang the app.
const LIMIT: u16 = 5000;

fn zone_of(e: &Event) -> Zone {
    if e.all_day {
        return Zone::Utc;
    }
    match e.timezone.parse::<chrono_tz::Tz>() {
        Ok(tz) => Zone::Iana(tz),
        Err(_) => Zone::Utc,
    }
}

fn rtz(z: &Zone) -> RTz {
    match z {
        Zone::Iana(tz) => RTz::Tz(*tz),
        Zone::Local => RTz::LOCAL,
        _ => RTz::UTC,
    }
}

/// Rewrite UNTIL=… to the UTC "Z" form.
fn normalise_until(rule: &str, zone: &Zone) -> String {
    rule.split(';')
        .map(|part| {
            let Some(v) = part.strip_prefix("UNTIL=").or_else(|| part.strip_prefix("until=")) else {
                return part.to_string();
            };
            if v.ends_with('Z') {
                return part.to_string();
            }
            let naive = if v.len() == 8 {
                NaiveDate::parse_from_str(v, "%Y%m%d")
                    .ok()
                    .map(|d| if matches!(zone, Zone::Utc) { d.and_time(NaiveTime::MIN) } else { d.and_hms_opt(23, 59, 59).unwrap_or_default() })
            } else {
                NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()
            };
            match naive {
                Some(n) => {
                    let ms = local_to_ms(n, zone);
                    format!("UNTIL={}", DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_default().format("%Y%m%dT%H%M%SZ"))
                }
                None => part.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// Occurrence start times (unix ms) of a series master whose occurrence *overlaps* [from, to).
/// A non-recurring event yields its own start if it overlaps. Never panics; a rule the crate rejects falls back to
/// the first occurrence only (and is logged), so the event is still visible.
pub fn occurrences(e: &Event, from: i64, to: i64) -> Vec<i64> {
    let dur = (e.end - e.start).max(0);
    let Some(rule) = e.rrule.as_deref().filter(|r| !r.trim().is_empty()) else {
        return if e.start < to && e.start + dur.max(1) > from { vec![e.start] } else { vec![] };
    };
    let zone = zone_of(e);
    let tz = rtz(&zone);
    let Some(start) = DateTime::<Utc>::from_timestamp_millis(e.start) else { return vec![] };
    let dt_start = start.with_timezone(&tz);
    let parsed: Result<RRule<Unvalidated>, _> = normalise_until(rule, &zone).parse();
    let set = match parsed.map_err(|e| e.to_string()).and_then(|r| r.build(dt_start).map_err(|e| e.to_string())) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(uid = %e.uid, "recurrence rule rejected ({err}); showing the first occurrence only");
            return if e.start < to && e.start + dur.max(1) > from { vec![e.start] } else { vec![] };
        }
    };
    let mut set = set;
    for x in &e.exdates {
        if let Some(d) = DateTime::<Utc>::from_timestamp_millis(*x) {
            set = set.exdate(d.with_timezone(&tz));
        }
    }
    let lo = DateTime::<Utc>::from_timestamp_millis(from - dur).unwrap_or(start).with_timezone(&tz);
    let hi = DateTime::<Utc>::from_timestamp_millis(to).unwrap_or(start).with_timezone(&tz);
    let res = set.after(lo).before(hi).all(LIMIT);
    let ex: std::collections::HashSet<i64> = e.exdates.iter().copied().collect();
    res.dates
        .into_iter()
        .map(|d| d.timestamp_millis())
        .filter(|s| !ex.contains(s) && *s < to && *s + dur.max(1) > from)
        .collect()
}

/// Floating expansion for VTIMEZONE observance onsets (no zone; naive values treated as UTC), up to `until`.
pub fn expand_naive(start: NaiveDateTime, rule: &str, until: NaiveDateTime) -> Vec<NaiveDateTime> {
    let dt_start = RTz::UTC.from_utc_datetime(&start);
    let Ok(r) = normalise_until(rule, &Zone::Utc).parse::<RRule<Unvalidated>>() else { return vec![] };
    let Ok(set) = r.build(dt_start) else { return vec![] };
    let hi = RTz::UTC.from_utc_datetime(&until);
    set.before(hi).all(LIMIT).dates.into_iter().map(|d| d.naive_utc()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(y: i32, m: u32, d: u32, h: u32, mi: u32) -> i64 {
        Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap().timestamp_millis()
    }

    fn weekly_london() -> Event {
        Event {
            uid: "w".into(),
            // Monday 2 March 2026 09:00 GMT (= 09:00 UTC)
            start: ms(2026, 3, 2, 9, 0),
            end: ms(2026, 3, 2, 10, 0),
            timezone: "Europe/London".into(),
            rrule: Some("FREQ=WEEKLY;BYDAY=MO".into()),
            ..Default::default()
        }
    }

    #[test]
    fn keeps_wall_clock_across_dst() {
        // UK clocks go forward on Sunday 29 March 2026: Monday 30 March 09:00 BST = 08:00 UTC.
        let occ = occurrences(&weekly_london(), ms(2026, 3, 1, 0, 0), ms(2026, 4, 1, 0, 0));
        assert_eq!(occ, vec![ms(2026, 3, 2, 9, 0), ms(2026, 3, 9, 9, 0), ms(2026, 3, 16, 9, 0), ms(2026, 3, 23, 9, 0), ms(2026, 3, 30, 8, 0)]);
    }

    #[test]
    fn exdate_and_local_until_are_honoured() {
        let mut e = weekly_london();
        e.exdates = vec![ms(2026, 3, 9, 9, 0)];
        // Local (non-UTC) UNTIL, as many servers write it — normalised instead of rejected.
        e.rrule = Some("FREQ=WEEKLY;BYDAY=MO;UNTIL=20260316T090000".into());
        let occ = occurrences(&e, ms(2026, 1, 1, 0, 0), ms(2027, 1, 1, 0, 0));
        assert_eq!(occ, vec![ms(2026, 3, 2, 9, 0), ms(2026, 3, 16, 9, 0)]);
    }

    #[test]
    fn date_only_until_includes_the_whole_day_and_all_day_series() {
        let e = Event {
            start: ms(2026, 12, 24, 0, 0),
            end: ms(2026, 12, 25, 0, 0),
            all_day: true,
            rrule: Some("FREQ=DAILY;UNTIL=20261226".into()),
            ..Default::default()
        };
        let occ = occurrences(&e, ms(2026, 12, 1, 0, 0), ms(2027, 1, 1, 0, 0));
        assert_eq!(occ, vec![ms(2026, 12, 24, 0, 0), ms(2026, 12, 25, 0, 0), ms(2026, 12, 26, 0, 0)]);
    }

    #[test]
    fn overlap_catches_an_occurrence_that_started_before_the_window() {
        let mut e = weekly_london();
        e.end = e.start + 3 * 3_600_000;
        let occ = occurrences(&e, ms(2026, 3, 2, 11, 0), ms(2026, 3, 2, 12, 0));
        assert_eq!(occ, vec![ms(2026, 3, 2, 9, 0)]);
    }

    #[test]
    fn count_and_bad_rules() {
        let mut e = weekly_london();
        e.rrule = Some("FREQ=DAILY;COUNT=3".into());
        assert_eq!(occurrences(&e, 0, ms(2030, 1, 1, 0, 0)).len(), 3);
        e.rrule = Some("FREQ=NONSENSE".into());
        assert_eq!(occurrences(&e, 0, ms(2030, 1, 1, 0, 0)), vec![e.start]);
    }
}
