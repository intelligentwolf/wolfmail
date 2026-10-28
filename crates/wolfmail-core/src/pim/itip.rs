//! Meeting invitations in mail (iMIP, RFC 6047) and the answers to them (iTIP, RFC 5546).
//! Sources: RFC 6047 §2.4 (text/calendar part with a `method` parameter equal to the iCalendar METHOD), RFC 5546
//! §3.2.2 REQUEST, §3.2.3 REPLY (attendee → organizer, one ATTENDEE with PARTSTAT), §3.2.5 CANCEL; mail-parser 1.0
//! `Message::parts()` / `MessagePart::content_type()` / `decoded()` (src/view/message.rs:429, src/view/part.rs:526).
use super::model::*;
use super::{commands, ical, lines, store};
use crate::engine::Engine;
use crate::error::{ApiError, Result};
use crate::model::{Address, CalendarPart, DraftInput};
use mail_parser::MessageParser;

/// The first text/calendar (or .ics attachment) part → (METHOD, ics text).
pub fn calendar_part(raw: &[u8]) -> Option<(String, String)> {
    let msg = MessageParser::default().parse(raw)?;
    for part in msg.parts() {
        let Some(ct) = part.content_type() else { continue };
        let is_cal = ct.ctype().eq_ignore_ascii_case("text") && ct.subtype().is_some_and(|s| s.eq_ignore_ascii_case("calendar"))
            || ct.ctype().eq_ignore_ascii_case("application") && ct.subtype().is_some_and(|s| s.eq_ignore_ascii_case("ics"));
        if !is_cal {
            continue;
        }
        let text = String::from_utf8_lossy(&part.decoded()).into_owned();
        let method = ct
            .attribute("method")
            .map(|m| m.to_ascii_uppercase())
            .or_else(|| lines::parse(&text).first().and_then(|c| c.text("METHOD")).map(|m| m.to_ascii_uppercase()))
            .unwrap_or_else(|| "PUBLISH".into());
        if text.contains("BEGIN:VEVENT") {
            return Some((method, text));
        }
    }
    None
}

/// The event an invitation describes (the master if there are overrides).
pub fn invite_event(ics: &str) -> Option<Event> {
    let evs = ical::events_from_ics(ics, "");
    evs.iter().find(|e| e.recurrence_id.is_none()).or(evs.first()).cloned()
}

async fn my_emails(e: &Engine) -> Vec<String> {
    e.store()
        .call(|c| {
            let mut st = c.prepare("SELECT DISTINCT lower(email) FROM identities")?;
            let v = st.query_map([], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(v)
        })
        .await
        .unwrap_or_default()
}

/// Build the reader's invite card. Never fails the message view: any problem just means no card.
pub async fn invite_for(e: &Engine, raw: &[u8]) -> Option<Invite> {
    let (method, ics) = calendar_part(raw)?;
    let ev = invite_event(&ics)?;
    let mine = my_emails(e).await;
    let me = ev.attendees.iter().find(|a| mine.contains(&a.email));
    let (uid, rid) = (ev.uid.clone(), ev.recurrence_id);
    let existing = e.pim().store.call(move |c| store::find_by_uid_any(c, &uid, rid)).await.ok().flatten();
    let conflicts = commands::occurrences_between(e, ev.start, ev.end.max(ev.start + 1), None)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|o| existing.as_ref().is_none_or(|x| x.event.id != o.event_id))
        .filter(|o| o.status != "CANCELLED" && o.my_partstat.as_deref() != Some("DECLINED"))
        .take(5)
        .collect();
    Some(Invite {
        method: method.clone(),
        uid: ev.uid.clone(),
        title: ev.title.clone(),
        start: ev.start,
        end: ev.end,
        all_day: ev.all_day,
        location: ev.location.clone(),
        description: ev.description.clone(),
        organizer: ev.organizer.clone(),
        attendees: ev.attendees.clone(),
        recurrence_id: ev.recurrence_id,
        rrule: ev.rrule.clone(),
        sequence: ev.sequence,
        my_email: me.map(|a| a.email.clone()),
        my_partstat: existing
            .as_ref()
            .and_then(|x| x.event.attendees.iter().find(|a| mine.contains(&a.email)).map(|a| a.partstat.clone()))
            .or_else(|| me.map(|a| a.partstat.clone())),
        existing_event_id: existing.map(|x| x.event.id),
        conflicts,
        reply_from: if method == "REPLY" { ev.attendees.first().cloned() } else { None },
    })
}

impl Engine {
    /// Accept / tentatively accept / decline a REQUEST: update (or add) the event in a calendar and tell the
    /// organizer. Google and Microsoft calendars that already hold the invite answer through their own API (they
    /// notify the organizer themselves, so no second email is sent); everything else answers by iMIP REPLY mail
    /// from the identity that was invited.
    pub async fn invite_respond(&self, message_id: String, response: String, calendar_id: Option<String>) -> Result<()> {
        let partstat = match response.as_str() {
            "ACCEPTED" | "TENTATIVE" | "DECLINED" => response.clone(),
            _ => return Err(ApiError::invalid("response", "Choose accept, maybe or decline.")),
        };
        let rid: i64 = message_id.parse().map_err(|_| ApiError::not_found("The message"))?;
        let raw = self.raw_bytes(rid).await?;
        let (method, ics) = calendar_part(&raw).ok_or_else(|| ApiError::invalid("messageId", "This message has no invitation in it."))?;
        if method != "REQUEST" {
            return Err(ApiError::invalid("messageId", "Only invitations can be answered."));
        }
        let ev = invite_event(&ics).ok_or_else(|| ApiError::invalid("messageId", "The invitation could not be read."))?;
        let mine = my_emails(self).await;
        let me = ev
            .attendees
            .iter()
            .find(|a| mine.contains(&a.email))
            .cloned()
            .ok_or_else(|| ApiError::invalid("messageId", "None of your addresses is on this invitation, so WolfMail can't answer it for you."))?;
        let organizer = ev.organizer.clone().ok_or_else(|| ApiError::invalid("messageId", "The invitation has no organizer to reply to."))?;

        let (uid, recur) = (ev.uid.clone(), ev.recurrence_id);
        let existing = self.pim().store.call(move |c| store::find_by_uid_any(c, &uid, recur)).await?;
        let mut answered_by_provider = false;
        match existing {
            Some(row) => {
                let cal_id = row.event.calendar_id.clone();
                let acct = self.pim().store.call(move |c| store::account(c, &store::calendar(c, &cal_id)?.account_id)).await?;
                let mut updated = row.event.clone();
                for a in updated.attendees.iter_mut().filter(|a| a.email == me.email) {
                    a.partstat = partstat.clone();
                }
                match acct.kind {
                    PimKind::Microsoft => {
                        let target = row.remote_id.clone().ok_or_else(|| ApiError::server("That event hasn't synced with Microsoft yet."))?;
                        super::graph::respond(self, &acct, &target, &partstat).await?;
                        answered_by_provider = true;
                        let id = acct.id.clone();
                        self.pim_sync_account(&id).await?;
                    }
                    PimKind::Google => {
                        // Patching our attendee's responseStatus makes Google notify the organizer.
                        self.pim().store.call(move |c| store::put_event(c, &updated, row.remote_id.as_deref(), row.series_remote_id.as_deref(), row.etag.as_deref(), true)).await?;
                        answered_by_provider = true;
                        self.pim().wake.notify_one();
                    }
                    _ => {
                        self.pim().store.call(move |c| store::put_event(c, &updated, row.remote_id.as_deref(), row.series_remote_id.as_deref(), row.etag.as_deref(), true)).await?;
                        self.pim().wake.notify_one();
                    }
                }
            }
            None if partstat != "DECLINED" => {
                let target = match calendar_id {
                    Some(c) => c,
                    None => commands::default_calendar_for(self, Some(&me.email)).await?,
                };
                let tid = target.clone();
                let (cal, acct) = self
                    .pim()
                    .store
                    .call(move |c| {
                        let cal = store::calendar(c, &tid)?;
                        let acct = store::account(c, &cal.account_id)?;
                        Ok((cal, acct))
                    })
                    .await?;
                if cal.read_only {
                    return Err(ApiError::invalid("calendarId", "That calendar is read-only. Choose another calendar."));
                }
                let mut ins = ev.clone();
                ins.calendar_id = target;
                ins.id = store::new_id();
                for a in ins.attendees.iter_mut().filter(|a| a.email == me.email) {
                    a.partstat = partstat.clone();
                }
                if ins.reminders.is_empty() && !ins.all_day {
                    ins.reminders = vec![15];
                }
                if acct.kind == PimKind::Microsoft {
                    let remote = self.pim().store.call({ let t = ins.calendar_id.clone(); move |c| Ok(store::calendar_remote(c, &t)?.1) }).await?;
                    super::graph::create_event(self, &acct, &remote, &ins).await?;
                } else {
                    let dirty = acct.kind != PimKind::Local;
                    self.pim().store.call(move |c| store::put_event(c, &ins, None, None, None, dirty)).await?;
                    self.pim().wake.notify_one();
                }
            }
            None => {}
        }
        self.emit_pim_changed();
        if answered_by_provider {
            return Ok(());
        }
        // iMIP REPLY from the identity that was invited.
        let identity_id = self
            .store()
            .call({
                let em = me.email.clone();
                move |c| Ok(c.query_row("SELECT id FROM identities WHERE lower(email)=?1 LIMIT 1", [&em], |r| r.get::<_, String>(0))?)
            })
            .await?;
        let verb = match partstat.as_str() {
            "ACCEPTED" => "Accepted",
            "TENTATIVE" => "Tentative",
            _ => "Declined",
        };
        let who = me.name.clone().unwrap_or_else(|| me.email.clone());
        let draft = DraftInput {
            id: None,
            identity_id,
            to: vec![Address { email: organizer.email.clone(), name: organizer.name.clone() }],
            cc: vec![],
            bcc: vec![],
            subject: format!("{verb}: {}", ev.title),
            html: String::new(),
            text: format!("{who} has {} the invitation “{}”.", verb.to_lowercase(), ev.title),
            in_reply_to: None,
            references: vec![],
            reply_to_message_id: None,
            forward_of_message_id: None,
            attachments: vec![],
            send_at: None,
            calendar: Some(CalendarPart { method: "REPLY".into(), ics: ical::itip_reply(&ev, &me, &partstat) }),
        };
        self.send(draft).await?;
        Ok(())
    }

    /// Apply an organizer's CANCEL or an attendee's REPLY to the matching event in the user's calendars.
    pub async fn invite_apply(&self, message_id: String) -> Result<()> {
        let rid: i64 = message_id.parse().map_err(|_| ApiError::not_found("The message"))?;
        let raw = self.raw_bytes(rid).await?;
        let (method, ics) = calendar_part(&raw).ok_or_else(|| ApiError::invalid("messageId", "This message has no invitation in it."))?;
        let ev = invite_event(&ics).ok_or_else(|| ApiError::invalid("messageId", "The invitation could not be read."))?;
        let (uid, recur) = (ev.uid.clone(), ev.recurrence_id);
        let row = self
            .pim()
            .store
            .call(move |c| store::find_by_uid_any(c, &uid, recur))
            .await?
            .ok_or_else(|| ApiError::invalid("messageId", "That event isn't in any of your calendars."))?;
        let mut updated = row.event.clone();
        match method.as_str() {
            "CANCEL" => updated.status = "CANCELLED".into(),
            "REPLY" => {
                for r in &ev.attendees {
                    if let Some(a) = updated.attendees.iter_mut().find(|a| a.email == r.email) {
                        a.partstat = r.partstat.clone();
                    }
                }
            }
            _ => return Err(ApiError::invalid("messageId", "Nothing to apply from this message.")),
        }
        let cal_id = updated.calendar_id.clone();
        let acct = self.pim().store.call(move |c| store::account(c, &store::calendar(c, &cal_id)?.account_id)).await?;
        if acct.kind == PimKind::Microsoft {
            // Exchange applies cancellations and replies to the organizer's calendar itself; just refresh.
            return self.pim_sync_account(&acct.id).await;
        }
        let dirty = acct.kind != PimKind::Local;
        self.pim().store.call(move |c| store::put_event(c, &updated, row.remote_id.as_deref(), row.series_remote_id.as_deref(), row.etag.as_deref(), dirty)).await?;
        self.pim().wake.notify_one();
        self.emit_pim_changed();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_calendar_part_and_method() {
        let raw = b"From: s@x.org\r\nTo: me@y.org\r\nSubject: Invite\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nYou are invited\r\n--b\r\nContent-Type: text/calendar; method=REQUEST; charset=utf-8\r\n\r\nBEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:inv1\r\nDTSTART:20261101T100000Z\r\nDTEND:20261101T110000Z\r\nSUMMARY:Planning\r\nORGANIZER:mailto:s@x.org\r\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:me@y.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n--b--\r\n";
        let (m, ics) = calendar_part(raw).unwrap();
        assert_eq!(m, "REQUEST");
        let ev = invite_event(&ics).unwrap();
        assert_eq!(ev.title, "Planning");
        assert_eq!(ev.attendees[0].email, "me@y.org");
    }
}
