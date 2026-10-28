//! Calendar & contacts against a LOCAL Radicale (CalDAV + CardDAV): discovery → MKCALENDAR → create/edit/override/
//! delete events → server-side edits come back → contacts create/delete both ways → account removal.
//! Start Radicale with (127.0.0.1 only):
//!   docker run -d --rm --name wm-radicale -p 127.0.0.1:35232:5232 \
//!     -v "$PWD/crates/wolfmail-core/tests/fixtures/radicale:/config:ro" tomsquest/docker-radicale:latest
//! and run `WOLFMAIL_IT_RADICALE=http://127.0.0.1:35232 cargo test --test pim_radicale`. Skipped when unset.
use std::sync::Arc;
use wolfmail_core::engine::{Engine, EngineConfig, EventSink};
use wolfmail_core::pim::model::*;
use wolfmail_core::secrets::MemoryStore;

struct Nop;
impl EventSink for Nop {
    fn emit(&self, _: &str, _: serde_json::Value) {}
}

const USER: &str = "tester";
const PASS: &str = "testpass";

async fn raw(method: &str, url: &str, body: Option<String>, ctype: &str, depth: Option<&str>) -> (u16, String) {
    let c = reqwest::Client::new();
    let mut rq = c.request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), url).basic_auth(USER, Some(PASS));
    if let Some(d) = depth {
        rq = rq.header("Depth", d);
    }
    if let Some(b) = body {
        rq = rq.header("Content-Type", ctype).body(b);
    }
    let r = rq.send().await.unwrap();
    (r.status().as_u16(), r.text().await.unwrap_or_default())
}

/// .ics/.vcf member URLs in a collection (PROPFIND depth 1).
async fn members(coll: &str, ext: &str) -> Vec<String> {
    let (_, body) = raw("PROPFIND", coll, Some(r#"<d:propfind xmlns:d="DAV:"><d:prop><d:getetag/></d:prop></d:propfind>"#.into()), "application/xml", Some("1")).await;
    let re = regex::Regex::new(r"<(?:[a-zA-Z]+:)?href>([^<]+)</").unwrap();
    let base = url::Url::parse(coll).unwrap();
    re.captures_iter(&body).map(|c| base.join(&c[1]).unwrap().to_string()).filter(|h| h.ends_with(ext)).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn radicale_round_trip() {
    let Ok(base) = std::env::var("WOLFMAIL_IT_RADICALE") else {
        eprintln!("skipped: set WOLFMAIL_IT_RADICALE");
        return;
    };
    let base = base.trim_end_matches('/').to_string();
    wolfmail_core::tls::install_crypto_provider();
    // A fresh user home each run.
    let run = uuid::Uuid::new_v4().simple().to_string();
    let book_url = format!("{base}/{USER}/book-{run}/");
    // RFC 5689 extended MKCOL creates an address book (Radicale supports it); WolfMail itself doesn't create books.
    let (st, _) = raw(
        "MKCOL",
        &book_url,
        Some(r#"<?xml version="1.0"?><d:mkcol xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><d:set><d:prop><d:resourcetype><d:collection/><c:addressbook/></d:resourcetype><d:displayname>BOOKNAME</d:displayname></d:prop></d:set></d:mkcol>"#.replace("BOOKNAME", &format!("Book {run}"))),
        "application/xml",
        None,
    )
    .await;
    assert!((200..300).contains(&st), "MKCOL addressbook: {st}");

    let dir = tempfile::tempdir().unwrap();
    let e = Engine::start(
        EngineConfig { data_dir: dir.path().join("d"), cache_dir: dir.path().join("c"), app_version: "it".into() },
        Arc::new(MemoryStore::default()),
        Arc::new(Nop),
        Arc::new(|_: &str| Ok(())),
    )
    .await
    .unwrap();

    // Wrong password is reported on the password field, not saved.
    let bad = e
        .pim_account_add(NewPimAccount { kind: PimKind::Dav, name: "Radicale".into(), email: None, url: Some(base.clone()), username: Some(USER.into()), password: Some("nope".into()) })
        .await
        .unwrap_err();
    assert_eq!(bad.field.as_deref(), Some("password"), "{bad:?}");

    let acct = e
        .pim_account_add(NewPimAccount { kind: PimKind::Dav, name: "Radicale".into(), email: None, url: Some(base.clone()), username: Some(USER.into()), password: Some(PASS.into()) })
        .await
        .unwrap();
    assert_eq!(acct.status, "ok", "{:?}", acct.status_message);

    // MKCALENDAR through the engine.
    e.calendar_create(acct.id.clone(), format!("Work {run}"), None).await.unwrap();
    let cal = e.calendars_list().await.unwrap().into_iter().find(|c| c.name == format!("Work {run}")).expect("new calendar listed");
    let book = e.address_books_list().await.unwrap().into_iter().find(|b| b.name == format!("Book {run}")).expect("address book discovered");

    // Create a weekly event → pushed as one .ics.
    use chrono::TimeZone;
    let start = chrono::Utc.with_ymd_and_hms(2027, 1, 5, 9, 0, 0).unwrap().timestamp_millis(); // a Tuesday
    let ev = e
        .event_save(
            Event {
                calendar_id: cal.id.clone(),
                title: "Standup".into(),
                start,
                end: start + 900_000,
                timezone: "Europe/London".into(),
                rrule: Some("FREQ=WEEKLY;COUNT=4".into()),
                reminders: vec![5],
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    e.pim_sync_now(Some(acct.id.clone())).await.unwrap();
    let coll = e.pim_accounts_list().await.unwrap().into_iter().find(|a| a.id == acct.id).unwrap();
    assert_eq!(coll.status, "ok", "{:?}", coll.status_message);
    // Find the collection URL from the server: the one .ics containing our UID.
    let cal_home = format!("{base}/{USER}/");
    let (_, listing) = raw("PROPFIND", &cal_home, Some(r#"<d:propfind xmlns:d="DAV:"><d:prop><d:displayname/></d:prop></d:propfind>"#.into()), "application/xml", Some("1")).await;
    assert!(listing.contains(&format!("Work {run}")), "calendar exists on the server");
    let mut found = None;
    let re = regex::Regex::new(r"<(?:[a-zA-Z]+:)?href>([^<]+)</").unwrap();
    for c in re.captures_iter(&listing) {
        let u = url::Url::parse(&cal_home).unwrap().join(&c[1]).unwrap().to_string();
        for m in members(&u, ".ics").await {
            let (_, body) = raw("GET", &m, None, "", None).await;
            if body.contains(&ev.uid) {
                found = Some((u.clone(), m, body));
            }
        }
    }
    let (cal_url, item_url, body) = found.expect("event stored on the server");
    assert!(body.contains("SUMMARY:Standup") && body.contains("RRULE:FREQ=WEEKLY;COUNT=4") && body.contains("TZID=Europe/London"));
    assert!(body.contains("BEGIN:VTIMEZONE"));

    // Edit one occurrence (the second) → the same resource now carries a RECURRENCE-ID override.
    let occ = e.events_range(start - 1, start + 30 * 86_400_000, Some(vec![cal.id.clone()])).await.unwrap();
    assert_eq!(occ.len(), 4);
    let second = occ[1].clone();
    let mut one = e.event_get(ev.id.clone()).await.unwrap();
    one.title = "Standup (moved)".into();
    one.start = second.start + 3_600_000;
    one.end = one.start + 900_000;
    e.event_save(one, Some(EditScope::This), Some(second.occurrence_start)).await.unwrap();
    e.pim_sync_now(Some(acct.id.clone())).await.unwrap();
    let (_, body) = raw("GET", &item_url, None, "", None).await;
    assert!(body.contains("RECURRENCE-ID") && body.contains("Standup (moved)"), "{body}");

    // A change made on the server comes back on the next sync.
    let changed = body.replace("SUMMARY:Standup\r\n", "SUMMARY:Daily sync\r\n");
    let (st, _) = raw("PUT", &item_url, Some(changed), "text/calendar", None).await;
    assert!((200..300).contains(&st));
    e.pim_sync_now(Some(acct.id.clone())).await.unwrap();
    assert_eq!(e.event_get(ev.id.clone()).await.unwrap().title, "Daily sync", "server edit pulled, local id kept");
    let occ = e.events_range(start - 1, start + 30 * 86_400_000, Some(vec![cal.id.clone()])).await.unwrap();
    assert_eq!(occ.len(), 4);
    assert!(occ.iter().any(|o| o.title == "Standup (moved)"));

    // Delete the series → gone from the server.
    e.event_delete(ev.id.clone(), EditScope::All, None).await.unwrap();
    e.pim_sync_now(Some(acct.id.clone())).await.unwrap();
    assert!(members(&cal_url, ".ics").await.is_empty(), "series deleted on the server");

    // Contacts: create here → appears there; delete there → disappears here.
    let ct = e
        .contact_save(Contact { book_id: book.id.clone(), full_name: "Jane Server".into(), emails: vec![Labeled { label: "work".into(), value: "jane@example.test".into() }], ..Default::default() })
        .await
        .unwrap();
    e.pim_sync_now(Some(acct.id.clone())).await.unwrap();
    let cards = members(&book_url, ".vcf").await;
    assert_eq!(cards.len(), 1);
    let (_, vcf) = raw("GET", &cards[0], None, "", None).await;
    assert!(vcf.contains("FN:Jane Server") && vcf.contains(&ct.uid));
    let (st, _) = raw("DELETE", &cards[0], None, "", None).await;
    assert!((200..300).contains(&st));
    e.pim_sync_now(Some(acct.id.clone())).await.unwrap();
    assert!(e.contacts_list(Some(book.id.clone()), None, None).await.unwrap().is_empty(), "server delete pulled");

    // A contact created on the server shows up here.
    let card = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:srv-1\r\nFN:Server Made\r\nEMAIL:made@example.test\r\nEND:VCARD\r\n";
    let (st, _) = raw("PUT", &format!("{book_url}srv-1.vcf"), Some(card.into()), "text/vcard", None).await;
    assert!((200..300).contains(&st));
    e.pim_sync_now(Some(acct.id.clone())).await.unwrap();
    let got = e.contacts_list(Some(book.id.clone()), Some("made".into()), None).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].full_name, "Server Made");

    e.pim_account_remove(acct.id.clone()).await.unwrap();
    assert!(e.calendars_list().await.unwrap().iter().all(|c| c.account_id != acct.id));
    // Leave the test server as we found it.
    raw("DELETE", &book_url, None, "", None).await;
    raw("DELETE", &cal_url, None, "", None).await;
}
