//! Sync one CalDAV/CardDAV account: discover (cached), list collections, push local edits, pull server changes.
//! Conflict rule (RFC 4918 §12.1 412 Precondition Failed on If-Match): the server copy wins and the user is told
//! which item, through a `pim://notice` event — an edit is never dropped silently.
use super::dav::{join, Collection, CollectionKind, Dav, Delta};
use super::model::*;
use super::{ical, store, vcard};
use crate::engine::Engine;
use crate::error::{ApiError, Result};
use std::collections::{HashMap, HashSet};

/// Default colours for calendars the server gives none (IntelligentWolf palette, light-theme contrast ≥ 3:1).
const PALETTE: &[&str] = &["#c41e1e", "#2563eb", "#15803d", "#b7791f", "#7c3aed", "#0e7490", "#be185d", "#4b4c53"];

fn safe_name(uid: &str) -> String {
    uid.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.@".contains(c) { c.to_string() } else { format!("%{:02X}", c as u32 & 0xff) }).collect()
}

pub async fn sync(e: &Engine, acct: &PimAccount) -> Result<()> {
    let pass = e.pim_password(acct)?;
    let dav = Dav { http: &e.inner.http, user: acct.username.clone().unwrap_or_default(), pass };
    let start = acct.url.clone().ok_or_else(|| ApiError::invalid("url", "This account has no server address."))?;
    let id = acct.id.clone();
    let st = e.pim().store.call(move |c| store::state(c, &id)).await?;
    let cached = |k: &str| st.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default();
    let (mut cal_homes, mut card_homes) = (cached("calHomes"), cached("cardHomes"));
    if cal_homes.is_empty() && card_homes.is_empty() {
        let (a, b) = dav.discover(&start).await?;
        cal_homes = a;
        card_homes = b;
        let v = serde_json::json!({ "calHomes": cal_homes, "cardHomes": card_homes });
        let id = acct.id.clone();
        e.pim().store.call(move |c| store::set_state(c, &id, &v)).await?;
    }
    let mut colls: Vec<Collection> = Vec::new();
    for h in cal_homes.iter().chain(card_homes.iter()) {
        match dav.collections(h).await {
            Ok(cs) => {
                for c in cs {
                    if !colls.iter().any(|x| super::dav::same_href(&x.url, &c.url)) {
                        colls.push(c);
                    }
                }
            }
            Err(err) => {
                // The home moved (server reconfigured): forget the cache so the next sync rediscovers.
                let id = acct.id.clone();
                e.pim().store.call(move |c| store::set_state(c, &id, &serde_json::json!({}))).await?;
                return Err(err);
            }
        }
    }

    // Register collections.
    let acct_id = acct.id.clone();
    let regs = colls.clone();
    let (cal_ids, book_ids) = e
        .pim()
        .store
        .call(move |c| {
            let mut cals = Vec::new();
            let mut books = Vec::new();
            for (i, col) in regs.iter().enumerate() {
                match col.kind {
                    CollectionKind::Calendar => {
                        let color = col.color.clone().unwrap_or_else(|| PALETTE[i % PALETTE.len()].to_string());
                        let id = store::upsert_calendar(c, &acct_id, &col.url, &col.name, &color, col.read_only, false)?;
                        cals.push((id, col.clone()));
                    }
                    CollectionKind::AddressBook => {
                        let id = store::upsert_book(c, &acct_id, &col.url, &col.name, col.read_only)?;
                        books.push((id, col.clone()));
                    }
                }
            }
            let keep_c: Vec<String> = cals.iter().map(|(_, c)| c.url.clone()).collect();
            let keep_b: Vec<String> = books.iter().map(|(_, c)| c.url.clone()).collect();
            store::prune_calendars(c, &acct_id, &keep_c)?;
            store::prune_books(c, &acct_id, &keep_b)?;
            Ok((cals, books))
        })
        .await?;

    push_events(e, &dav, acct).await?;
    push_contacts(e, &dav, acct).await?;

    for (cal_id, coll) in cal_ids {
        pull(e, &dav, &cal_id, &coll, true).await?;
    }
    for (book_id, coll) in book_ids {
        pull(e, &dav, &book_id, &coll, false).await?;
    }
    Ok(())
}

async fn pull(e: &Engine, dav: &Dav<'_>, local_id: &str, coll: &Collection, is_cal: bool) -> Result<()> {
    let lid = local_id.to_string();
    let (_, _, token, ctag) = e
        .pim()
        .store
        .call(move |c| if is_cal { store::calendar_remote(c, &lid) } else { store::book_remote(c, &lid) })
        .await?;
    // Nothing changed since last time (CalendarServer ctag) — skip the round trips.
    if token.is_some() && ctag.is_some() && ctag == coll.ctag {
        return Ok(());
    }
    let delta: Delta = dav.delta(coll, token.as_deref()).await?;
    let lid = local_id.to_string();
    let local: Vec<(String, Option<String>)> =
        e.pim().store.call(move |c| if is_cal { store::remote_ids(c, &lid) } else { store::contact_remote_ids(c, &lid) }).await?;
    let local_etag: HashMap<String, Option<String>> = local.iter().cloned().collect();
    let mut to_fetch = Vec::new();
    for (href, etag) in &delta.changed {
        let known = local_etag.iter().find(|(k, _)| super::dav::same_href(k, href)).map(|(_, v)| v.clone());
        if known.is_none() || etag.is_none() || known.flatten() != *etag {
            to_fetch.push(href.clone());
        }
    }
    let mut removed: Vec<String> = delta.removed.clone();
    if delta.full {
        for (href, _) in &local {
            if !delta.changed.iter().any(|(h, _)| super::dav::same_href(h, href)) {
                removed.push(href.clone());
            }
        }
    }
    let fetched = if to_fetch.is_empty() { Vec::new() } else { dav.multiget(coll, &to_fetch).await? };
    let lid = local_id.to_string();
    let (tok, ct) = (delta.token.clone(), coll.ctag.clone());
    e.pim()
        .store
        .call(move |c| {
            for (href, etag, data) in fetched {
                if is_cal {
                    let evs = ical::events_from_ics(&data, &lid);
                    let keep: HashSet<(String, Option<i64>)> = evs.iter().map(|x| (x.uid.clone(), x.recurrence_id)).collect();
                    for row in store::events_by_remote(c, &lid, &href)? {
                        if !row.dirty && !row.deleted && !keep.contains(&(row.event.uid.clone(), row.event.recurrence_id)) {
                            store::remove_event(c, &row.event.id)?;
                        }
                    }
                    for ev in evs {
                        store::upsert_remote_event(c, ev, &href, None, etag.as_deref())?;
                    }
                } else {
                    for ct in vcard::contacts_from_vcf(&data, &lid) {
                        store::upsert_remote_contact(c, ct, &href, etag.as_deref())?;
                    }
                }
            }
            for href in removed {
                if is_cal {
                    store::remove_events_by_remote(c, &lid, &href)?;
                } else {
                    store::remove_contacts_by_remote(c, &lid, &href)?;
                }
            }
            if is_cal {
                store::set_calendar_sync(c, &lid, tok.as_deref(), ct.as_deref())
            } else {
                store::set_book_sync(c, &lid, tok.as_deref(), ct.as_deref())
            }
        })
        .await
}

fn notice(e: &Engine, msg: String) {
    e.emit("pim://notice", serde_json::json!({ "message": msg }));
}

async fn push_events(e: &Engine, dav: &Dav<'_>, acct: &PimAccount) -> Result<()> {
    let id = acct.id.clone();
    let dirty = e.pim().store.call(move |c| store::dirty_events(c, &id)).await?;
    let mut groups: Vec<(String, String)> = Vec::new();
    for r in &dirty {
        let k = (r.event.calendar_id.clone(), r.event.uid.clone());
        if !groups.contains(&k) {
            groups.push(k);
        }
    }
    for (cal_id, uid) in groups {
        let (cid, u) = (cal_id.clone(), uid.clone());
        let (rows, coll_url) = e
            .pim()
            .store
            .call(move |c| Ok((store::events_by_uid(c, &cid, &u)?, store::calendar_remote(c, &cid)?.1)))
            .await?;
        let href = rows.iter().find_map(|r| r.remote_id.clone()).unwrap_or_else(|| join(&coll_url, &format!("{}.ics", safe_name(&uid))));
        let etag = rows.iter().find(|r| r.event.recurrence_id.is_none()).or(rows.first()).and_then(|r| r.etag.clone());
        let existed = rows.iter().any(|r| r.remote_id.is_some());
        let alive: Vec<&Event> = rows.iter().filter(|r| !r.deleted).map(|r| &r.event).collect();
        let title = rows.first().map(|r| r.event.title.clone()).unwrap_or_default();
        let res = if alive.is_empty() {
            if existed { dav.delete(&href, etag.as_deref()).await.map(|_| None) } else { Ok(None) }
        } else {
            dav.put(&href, ical::calendar_object(&alive, None), "text/calendar; charset=utf-8", if existed { etag.as_deref() } else { None }).await
        };
        let ids: Vec<(String, bool)> = rows.iter().map(|r| (r.event.id.clone(), r.deleted)).collect();
        match res {
            Ok(new_etag) => {
                let h = href.clone();
                e.pim()
                    .store
                    .call(move |c| {
                        for (id, deleted) in ids {
                            if deleted {
                                store::remove_event(c, &id)?;
                            } else {
                                // Unknown new etag: clear it so the next pull fetches the server's canonical copy.
                                c.execute("UPDATE events SET etag=NULL WHERE id=?1", [&id])?;
                                store::clear_event_dirty(c, &id, Some(&h), new_etag.as_deref())?;
                            }
                        }
                        Ok(())
                    })
                    .await?;
            }
            Err(err) if err.message.starts_with("CONFLICT") => {
                notice(e, format!("“{title}” was changed on the server at the same time. The server's version was kept — reopen it to make your change again."));
                e.pim()
                    .store
                    .call(move |c| {
                        for (id, _) in ids {
                            c.execute("UPDATE events SET dirty=0, deleted=0, etag=NULL WHERE id=?1", [&id])?;
                        }
                        Ok(())
                    })
                    .await?;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

async fn push_contacts(e: &Engine, dav: &Dav<'_>, acct: &PimAccount) -> Result<()> {
    let id = acct.id.clone();
    let dirty = e.pim().store.call(move |c| store::dirty_contacts(c, &id)).await?;
    for r in dirty {
        let bid = r.contact.book_id.clone();
        let coll_url = e.pim().store.call(move |c| Ok(store::book_remote(c, &bid)?.1)).await?;
        let href = r.remote_id.clone().unwrap_or_else(|| join(&coll_url, &format!("{}.vcf", safe_name(&r.contact.uid))));
        let res = if r.deleted {
            if r.remote_id.is_some() { dav.delete(&href, r.etag.as_deref()).await.map(|_| None) } else { Ok(None) }
        } else {
            dav.put(&href, vcard::vcard(&r.contact), "text/vcard; charset=utf-8", if r.remote_id.is_some() { r.etag.as_deref() } else { None }).await
        };
        let cid = r.contact.id.clone();
        let name = r.contact.full_name.clone();
        match res {
            Ok(new_etag) => {
                let deleted = r.deleted;
                e.pim()
                    .store
                    .call(move |c| {
                        if deleted {
                            store::remove_contact(c, &cid)
                        } else {
                            c.execute("UPDATE contacts SET etag=NULL WHERE id=?1", [&cid])?;
                            store::clear_contact_dirty(c, &cid, Some(&href), new_etag.as_deref())
                        }
                    })
                    .await?;
            }
            Err(err) if err.message.starts_with("CONFLICT") => {
                notice(e, format!("“{name}” was changed on the server at the same time. The server's version was kept."));
                e.pim().store.call(move |c| Ok(c.execute("UPDATE contacts SET dirty=0, deleted=0, etag=NULL WHERE id=?1", [&cid])?)).await?;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Create a calendar on the server (MKCALENDAR in the first calendar home).
pub async fn create_calendar(e: &Engine, acct: &PimAccount, name: &str) -> Result<()> {
    let pass = e.pim_password(acct)?;
    let dav = Dav { http: &e.inner.http, user: acct.username.clone().unwrap_or_default(), pass };
    let id = acct.id.clone();
    let st = e.pim().store.call(move |c| store::state(c, &id)).await?;
    let home = st
        .get("calHomes")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| ApiError::server("Sync this account once before adding calendars to it."))?;
    let slug = format!("{}/", super::store::new_id());
    let home = if home.ends_with('/') { home } else { format!("{home}/") };
    dav.mkcalendar(&join(&home, &slug), name).await
}
