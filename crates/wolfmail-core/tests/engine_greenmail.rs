//! End-to-end engine test against a LOCAL GreenMail (IMAP + SMTP): add account → sync → send → receive → read →
//! draft round trip → reply flags → trash + undo → search → undo-send cancel → remove.
//! Start GreenMail with:
//!   docker run -d --rm --name wm-greenmail -p 127.0.0.1:33143:3143 -p 127.0.0.1:33025:3025 \
//!     -e GREENMAIL_OPTS='-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.auth.disabled' \
//!     greenmail/standalone:2.1.3
//! and run `WOLFMAIL_IT_GREENMAIL=127.0.0.1 cargo test --test engine_greenmail`. Skipped when unset.
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
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
        if std::time::Instant::now() > deadline {
            panic!("timed out waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn inbox_query() -> ViewQuery {
    ViewQuery { folder_id: None, unified: Some(UnifiedView::Inbox), filter: None, search: None, threaded: false, sort: SortOrder::DateDesc, offset: 0, limit: 100 }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_round_trip() {
    let Ok(host) = std::env::var("WOLFMAIL_IT_GREENMAIL") else {
        eprintln!("skipped: set WOLFMAIL_IT_GREENMAIL");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Events::default());
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let o2 = Arc::clone(&opened);
    let engine = Engine::start(
        EngineConfig { data_dir: dir.path().join("data"), cache_dir: dir.path().join("cache"), app_version: "test".into() },
        Arc::new(MemoryStore::default()),
        events.clone(),
        Arc::new(move |u: &str| {
            o2.lock().push(u.to_string());
            Ok(())
        }),
    )
    .await
    .unwrap();
    engine.settings_set(json!({ "undoSendSeconds": 0 })).await.unwrap();

    let me = format!("me-{}@example.test", uuid::Uuid::new_v4().simple());
    let srv = |port| ServerConfig { host: host.clone(), port, security: Security::None, username: me.clone(), auth: AuthKind::Password };
    let acct = engine
        .account_add(NewAccount {
            name: "Test".into(),
            email: me.clone(),
            display_name: "Me Tester".into(),
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
    let folders = engine.folders_list(Some(acct.id.clone())).await.unwrap();
    assert!(folders.iter().any(|f| f.role == Some(FolderRole::Inbox)), "{folders:?}");

    // Send to ourselves with an attachment.
    let att = dir.path().join("notes.txt");
    std::fs::write(&att, "attached text").unwrap();
    let ident = acct.identities[0].clone();
    let draft = DraftInput {
        id: None,
        identity_id: ident.id.clone(),
        to: vec![Address { name: Some("Me".into()), email: me.clone() }],
        cc: vec![],
        bcc: vec![],
        subject: "Hello from WolfMail".into(),
        html: "<p>First <b>message</b></p>".into(),
        text: String::new(),
        in_reply_to: None,
        references: vec![],
        reply_to_message_id: None,
        forward_of_message_id: None,
        attachments: vec![DraftAttachment { path: Some(att.to_string_lossy().to_string()), from_message: None }],
        send_at: None, calendar: None,
    };
    let item = engine.send(draft.clone()).await.unwrap();
    let (e2, id) = (engine.clone(), item.id.clone());
    wait_for("outbox sent", 20, || {
        let (e, id) = (e2.clone(), id.clone());
        async move {
            let l = e.outbox_list().await.ok()?;
            let it = l.into_iter().find(|i| i.id == id)?;
            assert_ne!(it.state, OutboxState::Failed, "send failed: {:?}", it.error);
            (it.state == OutboxState::Sent).then_some(())
        }
    })
    .await;
    engine.sync_now(None).await.unwrap();
    let e2 = engine.clone();
    let msg = wait_for("delivery", 30, || {
        let e = e2.clone();
        async move {
            let _ = e.sync_now(None).await;
            e.messages_list(inbox_query()).await.ok()?.items.into_iter().find(|m| m.subject == "Hello from WolfMail")
        }
    })
    .await;
    assert!(!msg.flags.seen);
    let full = engine.message_get(msg.id.clone()).await.unwrap();
    assert!(full.html.as_deref().unwrap_or("").contains("<b>message</b>"), "{:?}", full.html);
    let a = full.attachments.iter().find(|a| a.filename == "notes.txt").expect("attachment listed");
    let saved = dir.path().join("saved.txt");
    engine.attachment_save(msg.id.clone(), a.id.clone(), saved.to_string_lossy().to_string()).await.unwrap();
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), "attached text");
    engine.attachment_open(msg.id.clone(), a.id.clone()).await.unwrap();
    assert!(opened.lock().iter().any(|p| p.ends_with("notes.txt")));
    assert!(engine.message_raw(msg.id.clone()).await.unwrap().contains("Subject: Hello from WolfMail"));

    // Flags.
    engine.messages_set_flags(vec![msg.id.clone()], vec!["seen".into(), "flagged".into()], vec![]).await.unwrap();
    let m = engine.messages_list(inbox_query()).await.unwrap().items.into_iter().find(|m| m.id == msg.id).unwrap();
    assert!(m.flags.seen && m.flags.flagged);

    // Draft round trip (local + server Drafts folder), then send it as a reply.
    let reply = DraftInput {
        subject: "Re: Hello from WolfMail".into(),
        html: "<p>Reply body</p>".into(),
        in_reply_to: full.summary.message_id.clone(),
        references: full.summary.message_id.clone().into_iter().collect(),
        reply_to_message_id: Some(msg.id.clone()),
        attachments: vec![],
        ..draft.clone()
    };
    let saved = engine.draft_save(reply.clone()).await.unwrap();
    assert!(saved.server_saved, "server draft: {:?}", saved.server_error);
    let back = engine.draft_get(saved.id.clone()).await.unwrap();
    assert_eq!(back.subject, "Re: Hello from WolfMail");
    // Saving again replaces the server copy instead of piling up drafts.
    let saved2 = engine.draft_save(DraftInput { html: "<p>Reply body v2</p>".into(), ..back.clone() }).await.unwrap();
    assert_eq!(saved2.id, saved.id);
    let drafts = engine.folders_list(Some(acct.id.clone())).await.unwrap().into_iter().find(|f| f.role == Some(FolderRole::Drafts)).unwrap();
    let e2 = engine.clone();
    let did = drafts.id.clone();
    let server_drafts = wait_for("draft synced", 20, || {
        let (e, did) = (e2.clone(), did.clone());
        async move {
            let _ = e.sync_now(None).await;
            let p = e
                .messages_list(ViewQuery { folder_id: Some(did), unified: None, threaded: false, ..inbox_query() })
                .await
                .ok()?;
            (p.total == 1).then_some(p)
        }
    })
    .await;
    // Opening the Drafts row gives back an editable draft that maps to the same local draft.
    let opened_draft = engine.draft_get(server_drafts.items[0].id.clone()).await.unwrap();
    assert_eq!(opened_draft.id, Some(saved.id.clone()));
    engine.send(DraftInput { id: Some(saved.id.clone()), ..back }).await.unwrap();
    let e2 = engine.clone();
    let mid = msg.id.clone();
    wait_for("answered flag", 30, || {
        let (e, mid) = (e2.clone(), mid.clone());
        async move {
            let m = e.messages_list(inbox_query()).await.ok()?.items.into_iter().find(|m| m.id == mid)?;
            m.flags.answered.then_some(())
        }
    })
    .await;

    // Search over headers + cached bodies.
    let e2 = engine.clone();
    wait_for("reply delivered", 30, || {
        let e = e2.clone();
        async move {
            let _ = e.sync_now(None).await;
            let h = e.search("subject:\"Re: Hello\"".into(), None, 10).await.ok()?;
            (!h.is_empty()).then_some(())
        }
    })
    .await;

    // Trash + undo.
    let before = events.0.lock().len();
    engine.messages_trash(vec![msg.id.clone()]).await.unwrap();
    let token = events.0.lock()[before..]
        .iter()
        .find(|(e, _)| e == "undoable")
        .map(|(_, p)| p["token"].as_str().unwrap().to_string())
        .expect("undoable event");
    assert!(engine.messages_list(inbox_query()).await.unwrap().items.iter().all(|m| m.subject != "Hello from WolfMail"));
    engine.messages_undo(token).await.unwrap();
    let e2 = engine.clone();
    wait_for("undo back in inbox", 30, || {
        let e = e2.clone();
        async move {
            let _ = e.sync_now(None).await;
            e.messages_list(inbox_query()).await.ok()?.items.into_iter().find(|m| m.subject == "Hello from WolfMail")
        }
    })
    .await;

    // Undo send: queue with a delay, cancel, get the draft back.
    engine.settings_set(json!({ "undoSendSeconds": 30 })).await.unwrap();
    let queued = engine.send(DraftInput { subject: "Never sent".into(), attachments: vec![], ..draft.clone() }).await.unwrap();
    let returned = engine.outbox_cancel(queued.id.clone()).await.unwrap();
    assert_eq!(returned.subject, "Never sent");

    // Address suggestions learnt from sent mail.
    let s = engine.contacts_suggest(me[..6].to_string(), 5).await.unwrap();
    assert!(s.iter().any(|a| a.email == me));

    // Status/counts events were emitted along the way.
    let names: Vec<String> = events.0.lock().iter().map(|(e, _)| e.clone()).collect();
    for e in ["sync://status", "mail://changed", "outbox://changed", "counts://changed"] {
        assert!(names.iter().any(|n| n == e), "no {e} event");
    }

    engine.account_remove(acct.id.clone()).await.unwrap();
    assert!(engine.accounts_list().await.unwrap().is_empty());
}
