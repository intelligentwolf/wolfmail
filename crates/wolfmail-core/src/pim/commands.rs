//! Calendar & contacts commands (docs/API.md "Calendar & contacts"). Every public `async fn` is one Tauri command.
//! Write rules per account kind:
//! - local: written straight to the store.
//! - dav, google: written to the store as `dirty` and pushed by the sync loop (woken immediately) — works offline.
//! - microsoft: Graph series edits need the server, so writes go to Graph directly and the calendar is re-read.
use super::model::*;
use super::{dav, graph, ical, recur, store, vcard};
use crate::engine::Engine;
use crate::error::{ApiError, ErrorKind, Result};
use crate::model::{Address, CalendarPart, DraftInput};
use crate::oauth::{self, OAuthProvider};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PimAccountPatch {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub url: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarPatch {
    pub name: Option<String>,
    pub color: Option<String>,
    pub visible: Option<bool>,
}

fn invalid(field: &str, msg: &str) -> ApiError {
    ApiError::invalid(field, msg)
}

/// Occurrences overlapping [from, to) in visible calendars (or the given ones), sorted by start.
pub async fn occurrences_between(e: &Engine, from: i64, to: i64, calendar_ids: Option<Vec<String>>) -> Result<Vec<Occurrence>> {
    Ok(occurrences_with_reminders(e, from, to, calendar_ids).await?.into_iter().map(|(o, _)| o).collect())
}

async fn occurrences_with_reminders(e: &Engine, from: i64, to: i64, calendar_ids: Option<Vec<String>>) -> Result<Vec<(Occurrence, Vec<i64>)>> {
    if to <= from {
        return Err(invalid("end", "The end of the range must be after the start."));
    }
    let mine: HashSet<String> = e
        .store()
        .call(|c| {
            let mut st = c.prepare("SELECT DISTINCT lower(email) FROM identities")?;
            let v = st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<HashSet<_>>>()?;
            Ok(v)
        })
        .await
        .unwrap_or_default();
    e.pim()
        .store
        .call(move |c| {
            let cals = store::calendars(c)?;
            let ids: Vec<String> = match calendar_ids {
                Some(ids) => ids,
                None => cals.iter().filter(|x| x.visible).map(|x| x.id.clone()).collect(),
            };
            let by_id: HashMap<String, Calendar> = cals.into_iter().map(|x| (x.id.clone(), x)).collect();
            let rows = store::events_for_range(c, &ids, from, to)?;
            // Overrides per series: (calendar, uid) → set of replaced occurrence starts.
            let mut replaced: HashMap<(String, String), HashSet<i64>> = HashMap::new();
            for r in rows.iter().filter(|r| r.event.recurrence_id.is_some()) {
                replaced.entry((r.event.calendar_id.clone(), r.event.uid.clone())).or_default().insert(r.event.recurrence_id.unwrap_or(0));
            }
            // Overrides outside the queried window still replace their occurrence; fetch them per series.
            let mut out = Vec::new();
            for r in &rows {
                let Some(cal) = by_id.get(&r.event.calendar_id) else { continue };
                let ev = &r.event;
                let my = ev.attendees.iter().find(|a| mine.contains(&a.email)).map(|a| a.partstat.clone());
                let mk = |start: i64, occ: i64| Occurrence {
                    event_id: ev.id.clone(),
                    calendar_id: ev.calendar_id.clone(),
                    title: if ev.title.is_empty() { "(no title)".into() } else { ev.title.clone() },
                    location: ev.location.clone(),
                    start,
                    end: start + (ev.end - ev.start).max(0),
                    all_day: ev.all_day,
                    color: cal.color.clone(),
                    recurring: ev.rrule.is_some() || ev.recurrence_id.is_some() || r.series_remote_id.is_some(),
                    occurrence_start: occ,
                    status: ev.status.clone(),
                    my_partstat: my.clone(),
                    read_only: cal.read_only,
                    has_attendees: !ev.attendees.is_empty(),
                };
                if ev.recurrence_id.is_some() {
                    if ev.status != "CANCELLED" && ev.start < to && ev.end.max(ev.start + 1) > from {
                        out.push((mk(ev.start, ev.recurrence_id.unwrap_or(ev.start)), ev.reminders.clone()));
                    }
                    continue;
                }
                if ev.rrule.is_some() {
                    let mut skip = replaced.get(&(ev.calendar_id.clone(), ev.uid.clone())).cloned().unwrap_or_default();
                    for x in store::events_by_uid(c, &ev.calendar_id, &ev.uid)? {
                        if let Some(rid) = x.event.recurrence_id {
                            skip.insert(rid);
                        }
                    }
                    for s in recur::occurrences(ev, from, to) {
                        if !skip.contains(&s) {
                            out.push((mk(s, s), ev.reminders.clone()));
                        }
                    }
                } else if ev.start < to && ev.end.max(ev.start + 1) > from {
                    out.push((mk(ev.start, ev.start), ev.reminders.clone()));
                }
            }
            out.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(b.0.all_day.cmp(&a.0.all_day)));
            Ok(out)
        })
        .await
}

/// The calendar new invitations/events go to: one belonging to an account with that email, else the default
/// writable one, else the local Home calendar.
pub async fn default_calendar_for(e: &Engine, email: Option<&str>) -> Result<String> {
    let em = email.map(|s| s.to_ascii_lowercase());
    e.pim()
        .store
        .call(move |c| {
            let cals = store::calendars(c)?;
            let accts = store::accounts(c)?;
            let writable: Vec<&Calendar> = cals.iter().filter(|x| !x.read_only).collect();
            if let Some(em) = &em {
                for a in accts.iter().filter(|a| a.email.as_deref().map(|x| x.to_ascii_lowercase()).as_ref() == Some(em)) {
                    if let Some(cal) = writable.iter().find(|x| x.account_id == a.id && x.is_default).or(writable.iter().find(|x| x.account_id == a.id)) {
                        return Ok(cal.id.clone());
                    }
                }
            }
            let local = accts.iter().find(|a| a.kind == PimKind::Local).map(|a| a.id.clone()).unwrap_or_default();
            writable
                .iter()
                .find(|x| x.account_id == local && x.is_default)
                .or(writable.iter().find(|x| x.is_default))
                .or(writable.first())
                .map(|x| x.id.clone())
                .ok_or_else(|| ApiError::new(ErrorKind::NotFound, "There is no calendar you can add events to."))
        })
        .await
}

pub async fn fire_due_reminders(e: &Engine) -> Result<()> {
    let now = crate::db::now_ms();
    // Reminders can be up to a week ahead; look that far for occurrences whose trigger time has come.
    let occs = occurrences_with_reminders(e, now - 10 * 60_000, now + 8 * 86_400_000, None).await?;
    for (o, mins) in occs {
        if o.status == "CANCELLED" || o.my_partstat.as_deref() == Some("DECLINED") {
            continue;
        }
        for m in mins {
            let trigger = o.start - m * 60_000;
            // Fire when due, but not for triggers that passed more than 10 minutes ago (app was closed).
            if trigger > now || trigger < now - 10 * 60_000 {
                continue;
            }
            let (id, occ) = (o.event_id.clone(), o.start);
            let fired = e.pim().store.call(move |c| store::reminder_fired(c, &id, occ, m)).await?;
            if fired {
                continue;
            }
            let (id, occ) = (o.event_id.clone(), o.start);
            e.pim().store.call(move |c| store::mark_reminder_fired(c, &id, occ, m)).await?;
            e.emit(
                "calendar://reminder",
                serde_json::json!({ "eventId": o.event_id, "title": o.title, "start": o.start, "allDay": o.all_day, "location": o.location, "minutes": m }),
            );
        }
    }
    Ok(())
}

impl Engine {
    // ── accounts ────────────────────────────────────────────────────────────

    pub async fn pim_accounts_list(&self) -> Result<Vec<PimAccount>> {
        self.pim().store.call(|c| store::accounts(c)).await
    }

    /// Sign in to Google/Microsoft for calendar & contacts (browser flow; separate grant from mail).
    pub async fn pim_oauth_authorize(&self, provider: OAuthProvider, email: String) -> Result<String> {
        self.oauth_browser_flow(provider, &email, oauth::pim_scopes(provider), &oauth::pim_key(&email)).await
    }

    pub async fn pim_account_add(&self, input: NewPimAccount) -> Result<PimAccount> {
        let name = input.name.trim().to_string();
        let mut acct = PimAccount {
            id: store::new_id(),
            kind: input.kind,
            name: name.clone(),
            email: input.email.as_ref().map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()),
            url: input.url.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            username: input.username.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
            enabled: true,
            status: "ok".into(),
            status_message: None,
            last_sync_at: None,
        };
        match acct.kind {
            PimKind::Local => return Err(invalid("kind", "This computer's calendar and contacts are already set up.")),
            PimKind::Dav => {
                let url = acct.url.clone().ok_or_else(|| invalid("url", "Enter the server address."))?;
                if !(url.starts_with("https://") || url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost")) {
                    return Err(invalid("url", "The server address must start with https:// so your password is sent securely."));
                }
                if acct.username.is_none() {
                    return Err(invalid("username", "Enter your user name."));
                }
                let pass = input.password.clone().filter(|p| !p.is_empty()).ok_or_else(|| invalid("password", "Enter your password (or app password)."))?;
                // Prove the address and password work before saving anything.
                let d = dav::Dav { http: &self.inner.http, user: acct.username.clone().unwrap_or_default(), pass: pass.clone() };
                d.discover(&url).await.map_err(|err| match err.kind {
                    ErrorKind::Auth => invalid("password", &err.message),
                    _ => invalid("url", &err.message),
                })?;
                self.inner.secrets.set(&super::password_key(&acct.id), &pass)?;
            }
            PimKind::Google | PimKind::Microsoft => {
                let email = acct.email.clone().ok_or_else(|| invalid("email", "Enter the account's email address."))?;
                self.pim_token(&acct).await.map_err(|_| {
                    ApiError::new(ErrorKind::Auth, format!("Sign in to {} first, then add the account.", if acct.kind == PimKind::Google { "Google" } else { "Microsoft" }))
                })?;
                if acct.name.is_empty() {
                    acct.name = email;
                }
            }
        }
        if acct.name.is_empty() {
            acct.name = acct.url.clone().unwrap_or_else(|| "Calendar".into());
        }
        let a = acct.clone();
        self.pim().store.call(move |c| store::insert_account(c, &a)).await?;
        let id = acct.id.clone();
        if let Err(err) = self.pim_sync_account(&id).await {
            tracing::info!("first calendar sync: {}", err.message);
        }
        let id = acct.id.clone();
        self.pim().store.call(move |c| store::account(c, &id)).await
    }

    pub async fn pim_account_update(&self, id: String, patch: PimAccountPatch, password: Option<String>) -> Result<PimAccount> {
        let i = id.clone();
        let cur = self.pim().store.call(move |c| store::account(c, &i)).await?;
        if let Some(p) = password.filter(|p| !p.is_empty()) {
            if cur.kind != PimKind::Dav {
                return Err(invalid("password", "This account signs in through its provider, not with a password."));
            }
            self.inner.secrets.set(&super::password_key(&id), &p)?;
        }
        let i = id.clone();
        self.pim()
            .store
            .call(move |c| {
                if let Some(n) = patch.name.filter(|n| !n.trim().is_empty()) {
                    c.execute("UPDATE pim_accounts SET name=?2 WHERE id=?1", rusqlite::params![i, n.trim()])?;
                }
                if let Some(en) = patch.enabled {
                    c.execute("UPDATE pim_accounts SET enabled=?2 WHERE id=?1", rusqlite::params![i, en as i64])?;
                }
                if let Some(u) = patch.url.filter(|u| !u.trim().is_empty()) {
                    c.execute("UPDATE pim_accounts SET url=?2, state='{}' WHERE id=?1", rusqlite::params![i, u.trim()])?;
                }
                if let Some(u) = patch.username.filter(|u| !u.trim().is_empty()) {
                    c.execute("UPDATE pim_accounts SET username=?2 WHERE id=?1", rusqlite::params![i, u.trim()])?;
                }
                Ok(())
            })
            .await?;
        self.pim().wake.notify_one();
        self.emit_pim_changed();
        let i = id.clone();
        self.pim().store.call(move |c| store::account(c, &i)).await
    }

    pub async fn pim_account_remove(&self, id: String) -> Result<()> {
        let i = id.clone();
        let acct = self.pim().store.call(move |c| store::account(c, &i)).await?;
        if acct.kind == PimKind::Local {
            return Err(invalid("id", "This computer's calendar can't be removed; hide its calendars instead."));
        }
        let _ = self.inner.secrets.delete(&super::password_key(&id));
        if let Some(em) = &acct.email {
            // Only drop the calendar grant if no other calendar account uses the same address.
            let em2 = em.clone();
            let others: i64 = self
                .pim()
                .store
                .call(move |c| Ok(c.query_row("SELECT COUNT(*) FROM pim_accounts WHERE lower(email)=?1", [em2], |r| r.get(0))?))
                .await?;
            if others <= 1 {
                let _ = self.inner.secrets.delete(&oauth::pim_key(em));
            }
        }
        self.pim().store.call(move |c| Ok(c.execute("DELETE FROM pim_accounts WHERE id=?1", [id])?)).await?;
        self.emit_pim_changed();
        Ok(())
    }

    pub async fn pim_sync_now(&self, account_id: Option<String>) -> Result<()> {
        match account_id {
            Some(id) => self.pim_sync_account(&id).await,
            None => {
                self.pim().wake.notify_one();
                Ok(())
            }
        }
    }

    // ── calendars ───────────────────────────────────────────────────────────

    pub async fn calendars_list(&self) -> Result<Vec<Calendar>> {
        self.pim().store.call(|c| store::calendars(c)).await
    }

    pub async fn calendar_update(&self, id: String, patch: CalendarPatch) -> Result<Calendar> {
        if let Some(col) = &patch.color {
            if !(col.len() == 7 && col.starts_with('#') && col[1..].chars().all(|c| c.is_ascii_hexdigit())) {
                return Err(invalid("color", "Pick a colour."));
            }
        }
        let r = self
            .pim()
            .store
            .call(move |c| {
                if let Some(n) = patch.name.filter(|n| !n.trim().is_empty()) {
                    c.execute("UPDATE calendars SET name=?2 WHERE id=?1", rusqlite::params![id, n.trim()])?;
                }
                if let Some(col) = patch.color {
                    c.execute("UPDATE calendars SET color=?2 WHERE id=?1", rusqlite::params![id, col])?;
                }
                if let Some(v) = patch.visible {
                    c.execute("UPDATE calendars SET visible=?2 WHERE id=?1", rusqlite::params![id, v as i64])?;
                }
                store::calendar(c, &id)
            })
            .await?;
        self.emit_pim_changed();
        Ok(r)
    }

    pub async fn calendar_create(&self, account_id: String, name: String, color: Option<String>) -> Result<()> {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(invalid("name", "Give the calendar a name."));
        }
        let a = account_id.clone();
        let acct = self.pim().store.call(move |c| store::account(c, &a)).await?;
        match acct.kind {
            PimKind::Local => {
                let col = color.unwrap_or_else(|| "#2563eb".into());
                self.pim().store.call(move |c| store::upsert_calendar(c, &account_id, &store::new_id(), &name, &col, false, false).map(|_| ())).await?;
            }
            PimKind::Dav => super::dav_sync::create_calendar(self, &acct, &name).await?,
            PimKind::Google => super::google::create_calendar(self, &acct, &name).await?,
            PimKind::Microsoft => graph::create_calendar(self, &acct, &name).await?,
        }
        if acct.kind != PimKind::Local {
            self.pim_sync_account(&acct.id).await?;
        }
        self.emit_pim_changed();
        Ok(())
    }

    /// Only calendars on this computer can be deleted here; server calendars are managed at the provider.
    pub async fn calendar_delete(&self, id: String) -> Result<()> {
        self.pim()
            .store
            .call(move |c| {
                let cal = store::calendar(c, &id)?;
                let acct = store::account(c, &cal.account_id)?;
                if acct.kind != PimKind::Local {
                    return Err(invalid("id", "Delete server calendars with your provider; WolfMail can hide them instead."));
                }
                let n: i64 = c.query_row("SELECT COUNT(*) FROM calendars WHERE account_id=?1", [&acct.id], |r| r.get(0))?;
                if n <= 1 {
                    return Err(invalid("id", "Keep at least one calendar on this computer."));
                }
                c.execute("DELETE FROM calendars WHERE id=?1", [&id])?;
                if cal.is_default {
                    c.execute("UPDATE calendars SET is_default=1 WHERE id=(SELECT id FROM calendars WHERE account_id=?1 LIMIT 1)", [&acct.id])?;
                }
                Ok(())
            })
            .await?;
        self.emit_pim_changed();
        Ok(())
    }

    // ── events ──────────────────────────────────────────────────────────────

    pub async fn events_range(&self, from: i64, to: i64, calendar_ids: Option<Vec<String>>) -> Result<Vec<Occurrence>> {
        if to - from > 400 * 86_400_000 {
            return Err(invalid("end", "Ask for at most about a year of events at a time."));
        }
        occurrences_between(self, from, to, calendar_ids).await
    }

    pub async fn event_get(&self, id: String) -> Result<Event> {
        self.pim()
            .store
            .call(move |c| {
                let r = store::event(c, &id)?;
                let cal = store::calendar(c, &r.event.calendar_id)?;
                let mut ev = r.event;
                ev.read_only = cal.read_only;
                Ok(ev)
            })
            .await
    }

    async fn calendar_ctx(&self, calendar_id: &str) -> Result<(Calendar, PimAccount, String)> {
        let id = calendar_id.to_string();
        self.pim()
            .store
            .call(move |c| {
                let cal = store::calendar(c, &id)?;
                let acct = store::account(c, &cal.account_id)?;
                let remote = store::calendar_remote(c, &id)?.1;
                Ok((cal, acct, remote))
            })
            .await
    }

    /// Create or change an event. For a recurring series, `scope` says whether the change applies to the
    /// occurrence that started at `occurrence_start` or to the whole series (times are applied as a shift).
    pub async fn event_save(&self, mut input: Event, scope: Option<EditScope>, occurrence_start: Option<i64>) -> Result<Event> {
        if input.end < input.start {
            return Err(invalid("end", "The event must end after it starts."));
        }
        if input.all_day {
            input.timezone.clear();
        } else if !input.timezone.is_empty() && input.timezone.parse::<chrono_tz::Tz>().is_err() {
            return Err(invalid("timezone", "Pick a time zone from the list."));
        }
        if let Some(r) = &input.rrule {
            let probe = Event { start: input.start, end: input.end, rrule: Some(r.clone()), timezone: input.timezone.clone(), all_day: input.all_day, ..Default::default() };
            if r.trim().is_empty() || r.parse::<rrule::RRule<rrule::Unvalidated>>().is_err() || recur::occurrences(&probe, input.start, input.start + 1).is_empty() {
                return Err(invalid("rrule", "That repeat rule isn't valid."));
            }
        }
        for a in &input.attendees {
            if a.email.parse::<lettre::Address>().is_err() {
                return Err(invalid("attendees", &format!("“{}” isn't an email address.", a.email)));
            }
        }
        let (cal, acct, remote_cal) = self.calendar_ctx(&input.calendar_id).await?;
        if cal.read_only {
            return Err(invalid("calendarId", "That calendar is read-only."));
        }
        self.ensure_organizer(&mut input, &acct).await;

        if input.id.is_empty() {
            input.id = store::new_id();
            input.uid = format!("{}@wolfmail", uuid::Uuid::new_v4());
            input.sequence = 0;
            input.recurrence_id = None;
            if acct.kind == PimKind::Microsoft {
                graph::create_event(self, &acct, &remote_cal, &input).await?;
                self.pim_sync_account(&acct.id).await?;
            } else {
                let (ev, dirty) = (input.clone(), acct.kind != PimKind::Local);
                self.pim().store.call(move |c| store::put_event(c, &ev, None, None, None, dirty)).await?;
                self.after_local_write(&acct).await;
                self.send_invitations(&input, &acct, "REQUEST", None).await?;
            }
            self.emit_pim_changed();
            return Ok(input);
        }

        let iid = input.id.clone();
        let row = self.pim().store.call(move |c| store::event(c, &iid)).await?;
        let old = row.event.clone();

        // Moving to another calendar = delete there + create here (same UID).
        if old.calendar_id != input.calendar_id {
            if old.rrule.is_some() || old.recurrence_id.is_some() || row.series_remote_id.is_some() {
                return Err(invalid("calendarId", "Repeating events can't be moved to another calendar. Copy it as a new event instead."));
            }
            self.event_delete(old.id.clone(), EditScope::All, None).await?;
            input.id = String::new();
            return Box::pin(self.event_save(input, None, None)).await;
        }

        let series = old.rrule.is_some() || old.recurrence_id.is_some() || row.series_remote_id.is_some();
        let scope = if series { scope.unwrap_or(EditScope::All) } else { EditScope::All };
        if !input.attendees.is_empty() {
            input.sequence = old.sequence + 1;
        }

        if acct.kind == PimKind::Microsoft {
            let (target, shift) = match (scope, &row.series_remote_id) {
                (EditScope::All, Some(master)) => (master.clone(), Some(input.start - occurrence_start.unwrap_or(old.start))),
                _ => (row.remote_id.clone().ok_or_else(|| ApiError::server("That event hasn't synced with Microsoft yet."))?, None),
            };
            graph::update_event(self, &acct, &target, &input, shift).await?;
            self.pim_sync_account(&acct.id).await?;
            self.emit_pim_changed();
            return Ok(input);
        }

        let dirty = acct.kind != PimKind::Local;
        let saved = if !series {
            let mut ev = input.clone();
            ev.uid = old.uid.clone();
            let r = (row.remote_id.clone(), row.series_remote_id.clone(), row.etag.clone());
            let ev2 = ev.clone();
            self.pim().store.call(move |c| store::put_event(c, &ev2, r.0.as_deref(), r.1.as_deref(), r.2.as_deref(), dirty)).await?;
            ev
        } else {
            // Find the master of this series.
            let (cal_id, uid) = (old.calendar_id.clone(), old.uid.clone());
            let rows = self.pim().store.call(move |c| store::events_by_uid(c, &cal_id, &uid)).await?;
            let master = rows.iter().find(|r| r.event.recurrence_id.is_none()).ok_or_else(|| ApiError::server("The rest of this repeating event isn't on this computer yet; sync and try again."))?;
            let occ = old.recurrence_id.or(occurrence_start).unwrap_or(old.start);
            match scope {
                EditScope::All => {
                    let shift = input.start - occurrence_start.or(old.recurrence_id).unwrap_or(master.event.start);
                    let mut m = input.clone();
                    m.id = master.event.id.clone();
                    m.uid = master.event.uid.clone();
                    m.recurrence_id = None;
                    m.start = master.event.start + shift;
                    m.end = m.start + (input.end - input.start);
                    if m.rrule.is_none() {
                        m.rrule = master.event.rrule.clone();
                    }
                    m.exdates = master.event.exdates.iter().map(|x| x + shift).collect();
                    let r = (master.remote_id.clone(), master.series_remote_id.clone(), master.etag.clone());
                    let m2 = m.clone();
                    self.pim().store.call(move |c| store::put_event(c, &m2, r.0.as_deref(), r.1.as_deref(), r.2.as_deref(), dirty)).await?;
                    m
                }
                EditScope::This => {
                    let existing = rows.iter().find(|r| r.event.recurrence_id == Some(occ));
                    let mut o = input.clone();
                    o.id = existing.map(|x| x.event.id.clone()).unwrap_or_else(store::new_id);
                    o.uid = master.event.uid.clone();
                    o.recurrence_id = Some(occ);
                    o.rrule = None;
                    o.exdates.clear();
                    let (rid, etag) = match existing {
                        Some(x) => (x.remote_id.clone(), x.etag.clone()),
                        // DAV keeps overrides in the master's resource; Google needs the master id to find it.
                        None => (if acct.kind == PimKind::Dav { master.remote_id.clone() } else { None }, if acct.kind == PimKind::Dav { master.etag.clone() } else { None }),
                    };
                    let series_remote = if acct.kind == PimKind::Google { master.remote_id.clone() } else { None };
                    let o2 = o.clone();
                    self.pim().store.call(move |c| store::put_event(c, &o2, rid.as_deref(), series_remote.as_deref(), etag.as_deref(), dirty)).await?;
                    if acct.kind == PimKind::Dav {
                        // The whole resource is re-PUT; mark the master dirty too so it carries the override.
                        let mid = master.event.id.clone();
                        self.pim().store.call(move |c| Ok(c.execute("UPDATE events SET dirty=1 WHERE id=?1", [&mid])?)).await?;
                    }
                    o
                }
            }
        };
        self.after_local_write(&acct).await;
        self.send_invitations(&saved, &acct, "REQUEST", Some(&old)).await?;
        self.emit_pim_changed();
        Ok(saved)
    }

    pub async fn event_delete(&self, id: String, scope: EditScope, occurrence_start: Option<i64>) -> Result<()> {
        let iid = id.clone();
        let row = self.pim().store.call(move |c| store::event(c, &iid)).await?;
        let (cal, acct, _) = self.calendar_ctx(&row.event.calendar_id).await?;
        if cal.read_only {
            return Err(invalid("id", "That calendar is read-only."));
        }
        let ev = row.event.clone();
        let series = ev.rrule.is_some() || ev.recurrence_id.is_some() || row.series_remote_id.is_some();

        if acct.kind == PimKind::Microsoft {
            let target = match (scope, &row.series_remote_id) {
                (EditScope::All, Some(master)) => master.clone(),
                _ => row.remote_id.clone().ok_or_else(|| ApiError::server("That event hasn't synced with Microsoft yet."))?,
            };
            graph::delete_event(self, &acct, &target).await?;
            self.pim_sync_account(&acct.id).await?;
            self.emit_pim_changed();
            return Ok(());
        }

        let local = acct.kind == PimKind::Local;
        let (cal_id, uid) = (ev.calendar_id.clone(), ev.uid.clone());
        let rows = self.pim().store.call(move |c| store::events_by_uid(c, &cal_id, &uid)).await?;
        if series && scope == EditScope::This {
            let occ = ev.recurrence_id.or(occurrence_start).unwrap_or(ev.start);
            let master = rows.iter().find(|r| r.event.recurrence_id.is_none()).cloned();
            let kind = acct.kind;
            self.pim()
                .store
                .call(move |c| {
                    if let Some(mut m) = master {
                        if !m.event.exdates.contains(&occ) {
                            m.event.exdates.push(occ);
                        }
                        store::put_event(c, &m.event, m.remote_id.as_deref(), m.series_remote_id.as_deref(), m.etag.as_deref(), !local)?;
                    }
                    for r in rows.iter().filter(|r| r.event.recurrence_id == Some(occ)) {
                        // Google keeps an exception as its own item: delete it there; DAV/local just drop it.
                        if kind == PimKind::Google && r.remote_id.is_some() {
                            store::mark_event_deleted(c, &r.event.id)?;
                        } else {
                            store::remove_event(c, &r.event.id)?;
                        }
                    }
                    Ok(())
                })
                .await?;
        } else {
            let kind = acct.kind;
            let on_server = rows.iter().any(|x| x.remote_id.is_some());
            self.pim()
                .store
                .call(move |c| {
                    for r in &rows {
                        // What has to reach the server: DAV deletes the one resource (all rows of the UID share it);
                        // Google deletes the master, which takes its exceptions with it.
                        let needs_push = match kind {
                            PimKind::Local | PimKind::Microsoft => false,
                            PimKind::Dav => on_server,
                            PimKind::Google => r.remote_id.is_some() && r.event.recurrence_id.is_none(),
                        };
                        if needs_push {
                            store::mark_event_deleted(c, &r.event.id)?;
                        } else {
                            store::remove_event(c, &r.event.id)?;
                        }
                    }
                    Ok(())
                })
                .await?;
            self.send_invitations(&ev, &acct, "CANCEL", None).await?;
        }
        self.after_local_write(&acct).await;
        self.emit_pim_changed();
        Ok(())
    }

    /// Make sure an event with attendees has an organizer (the matching identity, else the first one).
    async fn ensure_organizer(&self, ev: &mut Event, acct: &PimAccount) {
        if ev.attendees.is_empty() || ev.organizer.is_some() {
            return;
        }
        let want = acct.email.clone();
        let id: Option<(String, String)> = self
            .store()
            .call(move |c| {
                let mut st = c.prepare("SELECT lower(email), name FROM identities ORDER BY is_default DESC")?;
                let v = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v.iter().find(|(e, _)| Some(e) == want.as_ref()).cloned().or(v.first().cloned()))
            })
            .await
            .ok()
            .flatten();
        if let Some((email, name)) = id {
            ev.organizer = Some(Organizer { email, name: if name.is_empty() { None } else { Some(name) } });
        }
    }

    async fn after_local_write(&self, acct: &PimAccount) {
        if acct.kind != PimKind::Local {
            self.pim().wake.notify_one();
        }
    }

    /// For events on this computer or a CalDAV server, WolfMail itself mails the attendees (iMIP REQUEST/CANCEL).
    /// Google and Microsoft send their own invitations (sendUpdates=all / Exchange), so nothing is sent for them.
    /// Only the organizer sends, and only when something attendees care about changed.
    async fn send_invitations(&self, ev: &Event, acct: &PimAccount, method: &str, before: Option<&Event>) -> Result<()> {
        if !matches!(acct.kind, PimKind::Local | PimKind::Dav) || ev.attendees.is_empty() {
            return Ok(());
        }
        let Some(org) = &ev.organizer else { return Ok(()) };
        let org_email = org.email.clone();
        let identity: Option<String> = self
            .store()
            .call(move |c| {
                Ok(rusqlite::OptionalExtension::optional(c.query_row("SELECT id FROM identities WHERE lower(email)=?1 LIMIT 1", [&org_email], |r| r.get::<_, String>(0)))?)
            })
            .await?;
        let Some(identity_id) = identity else { return Ok(()) };
        if let Some(b) = before {
            let same = b.start == ev.start && b.end == ev.end && b.title == ev.title && b.location == ev.location && b.rrule == ev.rrule && b.attendees.iter().map(|a| &a.email).eq(ev.attendees.iter().map(|a| &a.email)) && b.status == ev.status;
            if same {
                return Ok(());
            }
        }
        let to: Vec<Address> = ev
            .attendees
            .iter()
            .filter(|a| !a.email.eq_ignore_ascii_case(&org.email))
            .map(|a| Address { email: a.email.clone(), name: a.name.clone() })
            .collect();
        if to.is_empty() {
            return Ok(());
        }
        let mut sent = ev.clone();
        if method == "CANCEL" {
            sent.status = "CANCELLED".into();
            sent.sequence += 1;
        }
        let ics = ical::calendar_object(&[&sent], Some(method));
        let when = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ev.start).map(|d| d.format("%a %e %b %Y %H:%M UTC").to_string()).unwrap_or_default();
        let (subject, text) = match method {
            "CANCEL" => (format!("Cancelled: {}", ev.title), format!("“{}” on {when} has been cancelled.", ev.title)),
            _ if before.is_some() => (format!("Updated invitation: {}", ev.title), format!("“{}” has changed. It is now on {when}.\n\n{}", ev.title, ev.location)),
            _ => (format!("Invitation: {}", ev.title), format!("You're invited to “{}” on {when}.\n{}\n\n{}", ev.title, ev.location, ev.description)),
        };
        let draft = DraftInput {
            id: None,
            identity_id,
            to,
            cc: vec![],
            bcc: vec![],
            subject,
            html: String::new(),
            text,
            in_reply_to: None,
            references: vec![],
            reply_to_message_id: None,
            forward_of_message_id: None,
            attachments: vec![],
            send_at: None,
            calendar: Some(CalendarPart { method: method.to_string(), ics }),
        };
        self.send(draft).await?;
        Ok(())
    }

    // ── contacts ────────────────────────────────────────────────────────────

    pub async fn address_books_list(&self) -> Result<Vec<AddressBook>> {
        self.pim().store.call(|c| store::address_books(c)).await
    }

    pub async fn contacts_list(&self, book_id: Option<String>, query: Option<String>, limit: Option<i64>) -> Result<Vec<Contact>> {
        let limit = limit.unwrap_or(5000).clamp(1, 20000);
        self.pim().store.call(move |c| store::contacts_list(c, book_id.as_deref(), query.as_deref(), limit)).await
    }

    pub async fn contact_get(&self, id: String) -> Result<Contact> {
        self.pim()
            .store
            .call(move |c| {
                let r = store::contact(c, &id)?;
                let mut ct = r.contact;
                ct.read_only = store::address_book(c, &ct.book_id)?.read_only;
                Ok(ct)
            })
            .await
    }

    async fn book_ctx(&self, book_id: &str) -> Result<(AddressBook, PimAccount)> {
        let id = book_id.to_string();
        self.pim()
            .store
            .call(move |c| {
                let b = store::address_book(c, &id)?;
                let a = store::account(c, &b.account_id)?;
                Ok((b, a))
            })
            .await
    }

    pub async fn contact_save(&self, mut ct: Contact) -> Result<Contact> {
        ct.emails.retain(|e| !e.value.trim().is_empty());
        ct.phones.retain(|p| !p.value.trim().is_empty());
        ct.addresses.retain(|a| !a.value.trim().is_empty());
        if ct.full_name.trim().is_empty() {
            ct.full_name = format!("{} {}", ct.given_name.trim(), ct.family_name.trim()).trim().to_string();
        }
        if ct.full_name.is_empty() && ct.org.trim().is_empty() && ct.emails.is_empty() {
            return Err(invalid("fullName", "Enter a name, a company or an email address."));
        }
        for e in &ct.emails {
            if e.value.trim().parse::<lettre::Address>().is_err() {
                return Err(invalid("emails", &format!("“{}” isn't an email address.", e.value)));
            }
        }
        if let Some(b) = &ct.birthday {
            let ok = chrono::NaiveDate::parse_from_str(b, "%Y-%m-%d").is_ok() || (b.len() == 7 && b.starts_with("--"));
            if !ok {
                return Err(invalid("birthday", "Enter the birthday as a date."));
            }
        }
        let (book, acct) = self.book_ctx(&ct.book_id).await?;
        if book.read_only {
            return Err(invalid("bookId", "This address book is read-only in WolfMail (Google contacts are shown, not edited). Choose another address book."));
        }
        let existing = if ct.id.is_empty() {
            None
        } else {
            let id = ct.id.clone();
            Some(self.pim().store.call(move |c| store::contact(c, &id)).await?)
        };
        if let Some(ex) = &existing {
            if ex.contact.book_id != ct.book_id {
                return Err(invalid("bookId", "Contacts can't be moved between address books yet; create it in the other book instead."));
            }
            ct.uid = ex.contact.uid.clone();
        } else {
            ct.id = store::new_id();
            ct.uid = uuid::Uuid::new_v4().to_string();
        }
        if acct.kind == PimKind::Microsoft {
            graph::save_contact(self, &acct, existing.as_ref().and_then(|x| x.remote_id.as_deref()), &ct).await?;
            self.pim_sync_account(&acct.id).await?;
            return Ok(ct);
        }
        let dirty = acct.kind != PimKind::Local;
        let (c2, rid, etag) = (ct.clone(), existing.as_ref().and_then(|x| x.remote_id.clone()), existing.as_ref().and_then(|x| x.etag.clone()));
        self.pim().store.call(move |c| store::put_contact(c, &c2, rid.as_deref(), etag.as_deref(), dirty)).await?;
        self.after_local_write(&acct).await;
        self.emit_pim_changed();
        Ok(ct)
    }

    pub async fn contact_delete(&self, id: String) -> Result<()> {
        let i = id.clone();
        let row = self.pim().store.call(move |c| store::contact(c, &i)).await?;
        let (book, acct) = self.book_ctx(&row.contact.book_id).await?;
        if book.read_only {
            return Err(invalid("id", "This address book is read-only in WolfMail."));
        }
        match acct.kind {
            PimKind::Microsoft => {
                if let Some(r) = &row.remote_id {
                    graph::delete_contact(self, &acct, r).await?;
                }
                self.pim().store.call(move |c| store::remove_contact(c, &id)).await?;
            }
            PimKind::Dav if row.remote_id.is_some() => {
                self.pim().store.call(move |c| store::mark_contact_deleted(c, &id)).await?;
                self.after_local_write(&acct).await;
            }
            _ => self.pim().store.call(move |c| store::remove_contact(c, &id)).await?,
        }
        self.emit_pim_changed();
        Ok(())
    }

    /// Import a .vcf file into an address book; returns how many contacts were added.
    pub async fn contacts_import_vcf(&self, book_id: String, path: String) -> Result<i64> {
        let data = tokio::fs::read(&path).await?;
        let text = String::from_utf8_lossy(&data).into_owned();
        let found = vcard::contacts_from_vcf(&text, &book_id);
        if found.is_empty() {
            return Err(invalid("path", "No contacts were found in that file."));
        }
        let mut n = 0;
        for mut ct in found {
            ct.id = String::new();
            ct.book_id = book_id.clone();
            if self.contact_save(ct).await.is_ok() {
                n += 1;
            }
        }
        Ok(n)
    }

    pub async fn contacts_export_vcf(&self, ids: Vec<String>, path: String) -> Result<i64> {
        let cards = self
            .pim()
            .store
            .call(move |c| {
                let mut out = Vec::new();
                for id in ids {
                    out.push(vcard::vcard(&store::contact(c, &id)?.contact));
                }
                Ok(out)
            })
            .await?;
        let n = cards.len() as i64;
        tokio::fs::write(&path, cards.concat()).await?;
        Ok(n)
    }

    /// contacts_suggest (mail composer) blends in address-book contacts: names the user saved win, then
    /// addresses seen in mail.
    pub(crate) async fn pim_suggest_merge(&self, seen: Vec<Address>, prefix: &str, limit: i64) -> Vec<Address> {
        let q = prefix.to_string();
        let book: Vec<Address> = self
            .pim()
            .store
            .call(move |c| store::contacts_list(c, None, Some(&q), limit))
            .await
            .unwrap_or_default()
            .into_iter()
            .flat_map(|ct| {
                let name = if ct.full_name.is_empty() { None } else { Some(ct.full_name.clone()) };
                ct.emails.into_iter().map(move |e| Address { email: e.value.to_ascii_lowercase(), name: name.clone() })
            })
            .collect();
        let mut out: Vec<Address> = Vec::new();
        let in_book: HashSet<String> = book.iter().map(|a| a.email.clone()).collect();
        let in_seen: HashSet<String> = seen.iter().map(|a| a.email.to_ascii_lowercase()).collect();
        for a in book.iter().filter(|a| in_seen.contains(&a.email)) {
            out.push(a.clone());
        }
        for a in seen.into_iter().filter(|a| !in_book.contains(&a.email.to_ascii_lowercase())) {
            out.push(a);
        }
        for a in book.into_iter().filter(|a| !in_seen.contains(&a.email)) {
            out.push(a);
        }
        let mut uniq = HashSet::new();
        out.retain(|a| uniq.insert(a.email.to_ascii_lowercase()));
        out.truncate(limit.max(1) as usize);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EngineConfig, EventSink};
    use crate::secrets::MemoryStore;
    use std::sync::Arc;

    struct Nop;
    impl EventSink for Nop {
        fn emit(&self, _: &str, _: serde_json::Value) {}
    }

    async fn engine() -> (Engine, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let e = Engine::start(
            EngineConfig { data_dir: dir.path().join("d"), cache_dir: dir.path().join("c"), app_version: "t".into() },
            Arc::new(MemoryStore::default()),
            Arc::new(Nop),
            Arc::new(|_: &str| Ok(())),
        )
        .await
        .unwrap();
        (e, dir)
    }

    fn ms(y: i32, m: u32, d: u32, h: u32) -> i64 {
        use chrono::TimeZone;
        chrono::Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap().timestamp_millis()
    }

    #[tokio::test]
    async fn local_series_edit_this_and_all_and_delete() {
        let (e, _d) = engine().await;
        let cal = e.calendars_list().await.unwrap().into_iter().next().unwrap();
        let ev = e
            .event_save(
                Event { calendar_id: cal.id.clone(), title: "Gym".into(), start: ms(2026, 6, 1, 7), end: ms(2026, 6, 1, 8), rrule: Some("FREQ=DAILY;COUNT=5".into()), reminders: vec![10], ..Default::default() },
                None,
                None,
            )
            .await
            .unwrap();
        let occ = e.events_range(ms(2026, 6, 1, 0), ms(2026, 6, 10, 0), None).await.unwrap();
        assert_eq!(occ.len(), 5);
        // Move only the third occurrence an hour later.
        let third = occ[2].clone();
        let mut edit = e.event_get(ev.id.clone()).await.unwrap();
        edit.start = third.start + 3_600_000;
        edit.end = third.end + 3_600_000;
        edit.title = "Gym (late)".into();
        e.event_save(edit, Some(EditScope::This), Some(third.occurrence_start)).await.unwrap();
        let occ = e.events_range(ms(2026, 6, 1, 0), ms(2026, 6, 10, 0), None).await.unwrap();
        assert_eq!(occ.len(), 5);
        assert_eq!(occ.iter().filter(|o| o.title == "Gym (late)").count(), 1);
        assert!(occ.iter().any(|o| o.start == third.start + 3_600_000));
        // Delete the fourth occurrence only.
        let fourth = occ.iter().find(|o| o.start == ms(2026, 6, 4, 7)).unwrap().clone();
        e.event_delete(fourth.event_id.clone(), EditScope::This, Some(fourth.occurrence_start)).await.unwrap();
        assert_eq!(e.events_range(ms(2026, 6, 1, 0), ms(2026, 6, 10, 0), None).await.unwrap().len(), 4);
        // Shift the whole series by two hours from the first occurrence.
        let mut all = e.event_get(ev.id.clone()).await.unwrap();
        all.start = ms(2026, 6, 1, 9);
        all.end = ms(2026, 6, 1, 10);
        e.event_save(all, Some(EditScope::All), Some(ms(2026, 6, 1, 7))).await.unwrap();
        let occ = e.events_range(ms(2026, 6, 1, 0), ms(2026, 6, 10, 0), None).await.unwrap();
        assert!(occ.iter().any(|o| o.start == ms(2026, 6, 2, 9)));
        assert!(!occ.iter().any(|o| o.start == ms(2026, 6, 4, 9)), "excluded date moved with the series");
        // Delete everything.
        e.event_delete(ev.id.clone(), EditScope::All, None).await.unwrap();
        assert!(e.events_range(ms(2026, 6, 1, 0), ms(2026, 6, 10, 0), None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn contacts_crud_search_and_suggest() {
        let (e, dir) = engine().await;
        let book = e.address_books_list().await.unwrap().into_iter().next().unwrap();
        let c = e
            .contact_save(Contact { book_id: book.id.clone(), full_name: "Sarah Mitchell".into(), emails: vec![Labeled { label: "work".into(), value: "sarah@brightbloom.example".into() }], ..Default::default() })
            .await
            .unwrap();
        assert!(e.contact_save(Contact { book_id: book.id.clone(), emails: vec![Labeled { label: String::new(), value: "not-an-email".into() }], ..Default::default() }).await.is_err());
        assert_eq!(e.contacts_list(None, Some("mitch".into()), None).await.unwrap().len(), 1);
        assert_eq!(e.contacts_list(None, Some("brightbloom".into()), None).await.unwrap().len(), 1);
        let s = e.pim_suggest_merge(vec![], "sar", 5).await;
        assert_eq!(s[0].email, "sarah@brightbloom.example");
        let path = dir.path().join("out.vcf").to_string_lossy().to_string();
        assert_eq!(e.contacts_export_vcf(vec![c.id.clone()], path.clone()).await.unwrap(), 1);
        e.contact_delete(c.id.clone()).await.unwrap();
        assert!(e.contacts_list(None, None, None).await.unwrap().is_empty());
        assert_eq!(e.contacts_import_vcf(book.id.clone(), path).await.unwrap(), 1);
    }
}
