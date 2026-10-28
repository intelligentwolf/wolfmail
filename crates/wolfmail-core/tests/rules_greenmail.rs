//! Filters and snooze through the real engine against a LOCAL GreenMail (same container as engine_greenmail.rs):
//! new mail from a newsletter address is filed into a folder and tagged by a filter before it is announced;
//! a snoozed message disappears from the Inbox and its unread count, then comes back unread on time.
//! Run with `WOLFMAIL_IT_GREENMAIL=127.0.0.1 cargo test --test rules_greenmail`. Skipped when unset.
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use wolfmail_core::engine::rules::{FilterAction, FilterCondition, FilterField, FilterOp, FilterRule};
use wolfmail_core::engine::{Engine, EngineConfig, EventSink};
use wolfmail_core::model::*;
use wolfmail_core::secrets::MemoryStore;

#[derive(Default)]
struct Events(Mutex<Vec<(String, Value)>>);
impl EventSink for Events {
    fn emit(&self, event: &str, payload: Value) {
        self.0.lock().push((event.to_string(), payload));
    }
}

async fn wait_for<F, Fut, T>(what: &str, secs: u64, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Plain SMTP to GreenMail (auth disabled, localhost only) so the mail comes from a sender other than the account.
async fn deliver(host: &str, from: &str, to: &str, subject: &str) {
    let s = tokio::net::TcpStream::connect((host, 33025)).await.unwrap();
    let (r, mut w) = s.into_split();
    let mut r = BufReader::new(r);
    let mut line = String::new();
    async fn expect(r: &mut BufReader<tokio::net::tcp::OwnedReadHalf>, line: &mut String, code: &str) {
        loop {
            line.clear();
            r.read_line(line).await.unwrap();
            assert!(line.starts_with(code), "SMTP expected {code}, got {line}");
            if line.as_bytes().get(3) != Some(&b'-') {
                break;
            }
        }
    }
    expect(&mut r, &mut line, "220").await;
    for (cmd, code) in [
        ("EHLO test.local\r\n".to_string(), "250"),
        (format!("MAIL FROM:<{from}>\r\n"), "250"),
        (format!("RCPT TO:<{to}>\r\n"), "250"),
        ("DATA\r\n".to_string(), "354"),
    ] {
        w.write_all(cmd.as_bytes()).await.unwrap();
        expect(&mut r, &mut line, code).await;
    }
    let msg = format!(
        "From: Shop News <{from}>\r\nTo: <{to}>\r\nSubject: {subject}\r\nMessage-ID: <{}@test.local>\r\nDate: Wed, 28 Oct 2026 12:00:00 +0000\r\n\r\nHello\r\n.\r\n",
        uuid::Uuid::new_v4().simple()
    );
    w.write_all(msg.as_bytes()).await.unwrap();
    expect(&mut r, &mut line, "250").await;
    w.write_all(b"QUIT\r\n").await.unwrap();
}

fn inbox() -> ViewQuery {
    ViewQuery { folder_id: None, unified: Some(UnifiedView::Inbox), filter: None, search: None, threaded: false, sort: SortOrder::DateDesc, offset: 0, limit: 200 }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn filters_and_snooze() {
    let Ok(host) = std::env::var("WOLFMAIL_IT_GREENMAIL") else {
        eprintln!("skipped: set WOLFMAIL_IT_GREENMAIL");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Events::default());
    let engine = Engine::start(
        EngineConfig { data_dir: dir.path().join("data"), cache_dir: dir.path().join("cache"), app_version: "test".into() },
        Arc::new(MemoryStore::default()),
        events.clone(),
        Arc::new(|_: &str| Ok(())),
    )
    .await
    .unwrap();
    let me = format!("rules-{}@example.test", uuid::Uuid::new_v4().simple());
    let srv = |port| ServerConfig { host: host.clone(), port, security: Security::None, username: me.clone(), auth: AuthKind::Password };
    let acct = engine
        .account_add(NewAccount {
            name: "Rules".into(),
            email: me.clone(),
            display_name: "Rules Tester".into(),
            provider: Provider::Imap,
            imap: srv(33143),
            smtp: srv(33025),
            password: Some("x".into()),
            color: None,
        })
        .await
        .unwrap();
    let e2 = engine.clone();
    wait_for("first sync", 20, || {
        let e = e2.clone();
        async move { e.accounts_list().await.ok()?.into_iter().find(|a| a.status == AccountStatus::Ok) }
    })
    .await;

    // A folder for newsletters and a filter that files + tags them.
    let news = engine.folder_create(acct.id.clone(), None, "Newsletters".into()).await.unwrap();
    let rule = engine
        .filter_upsert(FilterRule {
            id: String::new(),
            name: "Newsletters".into(),
            enabled: true,
            account_id: Some(acct.id.clone()),
            match_all: true,
            conditions: vec![FilterCondition { field: FilterField::From, op: FilterOp::EndsWith, value: "@shop.example".into() }],
            actions: vec![FilterAction::Tag { tag: "$label5".into() }, FilterAction::MoveTo { folder_id: news.id.clone() }],
            sort_order: 0,
        })
        .await
        .unwrap();
    assert!(!rule.id.is_empty());
    // A move filter not tied to its folder's account is refused with a field error.
    let mut loose = rule.clone();
    loose.id = String::new();
    loose.account_id = None;
    assert_eq!(engine.filter_upsert(loose).await.unwrap_err().field.as_deref(), Some("actions"));

    deliver(&host, "deals@shop.example", &me, "Weekly deals").await;
    deliver(&host, "friend@example.com", &me, "Lunch on Friday?").await;

    let e2 = engine.clone();
    let filed = wait_for("newsletter filed by the filter", 40, || {
        let (e, fid) = (e2.clone(), news.id.clone());
        async move {
            let _ = e.sync_now(None).await;
            let q = ViewQuery { folder_id: Some(fid), unified: None, ..inbox() };
            e.messages_list(q).await.ok()?.items.into_iter().find(|m| m.subject == "Weekly deals")
        }
    })
    .await;
    assert!(filed.tags.iter().any(|t| t == "$label5"), "tag applied: {:?}", filed.tags);
    let e2 = engine.clone();
    let lunch = wait_for("other mail stays in the Inbox", 30, || {
        let e = e2.clone();
        async move { e.messages_list(inbox()).await.ok()?.items.into_iter().find(|m| m.subject == "Lunch on Friday?") }
    })
    .await;
    let inbox_now = engine.messages_list(inbox()).await.unwrap();
    assert!(inbox_now.items.iter().all(|m| m.subject != "Weekly deals"), "filed mail must leave the Inbox");
    // The filter's move must not offer an Undo toast; the new-mail announcement must not include the filed message.
    {
        let ev = events.0.lock();
        assert!(!ev.iter().any(|(n, _)| n == "undoable"), "no undo toast for automatic filter moves");
        for (n, p) in ev.iter().filter(|(n, _)| n == "mail://new") {
            let subjects: Vec<&str> = p["messages"].as_array().unwrap().iter().filter_map(|m| m["subject"].as_str()).collect();
            assert!(!subjects.contains(&"Weekly deals"), "{n}: filed mail was announced: {subjects:?}");
        }
    }

    // Snooze: hidden from the Inbox and its count, listed under Snoozed, back unread after the time.
    let inbox_folder = engine.folders_list(Some(acct.id.clone())).await.unwrap().into_iter().find(|f| f.role == Some(FolderRole::Inbox)).unwrap();
    let unread_before = inbox_folder.unread;
    assert!(engine.messages_snooze(vec![lunch.id.clone()], 0).await.is_err(), "past times are refused");
    let until = chrono_now_ms() + 31_000;
    engine.messages_snooze(vec![lunch.id.clone()], until).await.unwrap();
    let after = engine.messages_list(inbox()).await.unwrap();
    assert!(after.items.iter().all(|m| m.id != lunch.id), "snoozed mail is hidden from the Inbox");
    let snoozed = engine.messages_list(ViewQuery { unified: Some(UnifiedView::Snoozed), ..inbox() }).await.unwrap();
    let s = snoozed.items.iter().find(|m| m.id == lunch.id).expect("listed under Snoozed");
    assert_eq!(s.snoozed_until, Some(until));
    let f = engine.folders_list(Some(acct.id.clone())).await.unwrap().into_iter().find(|f| f.id == inbox_folder.id).unwrap();
    assert_eq!(f.unread, unread_before - 1, "snoozed unread mail is not counted");

    let e2 = engine.clone();
    let back = wait_for("snooze comes back", 45, || {
        let (e, id) = (e2.clone(), lunch.id.clone());
        async move { e.messages_list(inbox()).await.ok()?.items.into_iter().find(|m| m.id == id) }
    })
    .await;
    assert!(!back.flags.seen, "comes back unread");
    assert_eq!(back.snoozed_until, None);
    assert!(events.0.lock().iter().any(|(n, p)| n == "mail://new"
        && p["messages"].as_array().is_some_and(|a| a.iter().any(|m| m["id"].as_str() == Some(lunch.id.as_str())))));

    engine.filter_delete(rule.id).await.unwrap();
    assert!(engine.filters_list().await.unwrap().is_empty());
    engine.account_remove(acct.id).await.unwrap();
}

fn chrono_now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}
