//! Gmail API and Microsoft Graph accounts end to end through the real engine (runner, store, commands) against a
//! LOCAL wiremock server that emulates the endpoints WolfMail uses. Never touches Google or Microsoft.
//! The mock answers follow the documented shapes cited in src/providers/gmail.rs and src/providers/graph.rs.
use base64::Engine as _;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};
use wolfmail_core::engine::{Engine, EngineConfig, EventSink};
use wolfmail_core::model::*;
use wolfmail_core::oauth::{self, OAuthProvider};
use wolfmail_core::providers::ApiBases;
use wolfmail_core::secrets::MemoryStore;

#[derive(Default)]
struct Events(Mutex<Vec<(String, Value)>>);
impl EventSink for Events {
    fn emit(&self, event: &str, payload: Value) {
        self.0.lock().push((event.to_string(), payload));
    }
}
impl Events {
    fn find(&self, name: &str) -> Vec<Value> {
        self.0.lock().iter().filter(|(e, _)| e == name).map(|(_, v)| v.clone()).collect()
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
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn q_inbox() -> ViewQuery {
    ViewQuery { folder_id: None, unified: Some(UnifiedView::Inbox), filter: None, search: None, threaded: false, sort: SortOrder::DateDesc, offset: 0, limit: 100, keep_ids: Vec::new() }
}
fn q_folder(id: &str) -> ViewQuery {
    ViewQuery { folder_id: Some(id.into()), unified: None, filter: None, search: None, threaded: false, sort: SortOrder::DateDesc, offset: 0, limit: 100, keep_ids: Vec::new() }
}

fn raw_mime(subject: &str, mid: &str) -> String {
    format!("From: Ann <ann@example.com>\r\nTo: me@example.test\r\nSubject: {subject}\r\nMessage-ID: <{mid}>\r\nDate: Tue, 27 Oct 2026 10:00:00 +0000\r\nList-Unsubscribe: <https://lists.example.com/u?x=1>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello from {subject}.\r\n")
}

async fn engine(server: &MockServer) -> (Engine, Arc<Events>, Arc<MemoryStore>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let events = Arc::new(Events::default());
    let secrets = Arc::new(MemoryStore::default());
    let engine = Engine::start_with_bases(
        EngineConfig { data_dir: dir.path().join("data"), cache_dir: dir.path().join("cache"), app_version: "test".into() },
        ApiBases { gmail: format!("{}/gmail/v1", server.uri()), graph: format!("{}/graph", server.uri()) },
        secrets.clone(),
        events.clone(),
        Arc::new(|_: &str| Ok(())),
    )
    .await
    .unwrap();
    engine.settings_set(json!({ "undoSendSeconds": 0 })).await.unwrap();
    (engine, events, secrets, dir)
}

fn far_future() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64 + 3_600_000
}

fn draft(identity: &str, to: &str, subject: &str) -> DraftInput {
    DraftInput {
        id: None,
        identity_id: identity.into(),
        to: vec![Address { name: None, email: to.into() }],
        cc: vec![],
        bcc: vec![Address { name: None, email: "secret@example.com".into() }],
        subject: subject.into(),
        html: "<p>Body</p>".into(),
        text: "Body".into(),
        in_reply_to: None,
        references: vec![],
        reply_to_message_id: None,
        forward_of_message_id: None,
        attachments: vec![],
        send_at: None,
        calendar: None,
    }
}

// ------------------------------------------------------------------------------------------------ Gmail

#[derive(Default)]
struct GmailState {
    labels: Vec<(String, String, String)>, // id, name, type
    msgs: Vec<(String, String, Vec<String>, String)>, // id, threadId, labelIds, subject
    history_added: Vec<String>,
    drafts: BTreeSet<String>,
}

fn gmail_msg_json(st: &GmailState, id: &str, format: &str) -> Option<Value> {
    let (id, thread, labels, subject) = st.msgs.iter().find(|m| m.0 == id)?.clone();
    Some(match format {
        "minimal" => json!({ "id": id, "threadId": thread, "labelIds": labels }),
        "raw" => json!({ "id": id, "threadId": thread, "labelIds": labels,
                         "raw": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw_mime(&subject, &format!("{id}@example.com"))) }),
        _ => json!({ "id": id, "threadId": thread, "labelIds": labels, "snippet": format!("snippet {subject}"),
                     "internalDate": "1793091600000", "sizeEstimate": 900,
                     "payload": { "mimeType": "text/plain", "headers": [
                        { "name": "From", "value": "Ann <ann@example.com>" }, { "name": "To", "value": "me@gmail.com" },
                        { "name": "Subject", "value": subject }, { "name": "Message-ID", "value": format!("<{id}@example.com>") },
                        { "name": "Date", "value": "Tue, 27 Oct 2026 10:00:00 +0000" } ] } }),
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gmail_account_end_to_end() {
    let server = MockServer::start().await;
    let st = Arc::new(Mutex::new(GmailState {
        labels: vec![
            ("INBOX".into(), "INBOX".into(), "system".into()),
            ("SENT".into(), "SENT".into(), "system".into()),
            ("DRAFT".into(), "DRAFT".into(), "system".into()),
            ("TRASH".into(), "TRASH".into(), "system".into()),
            ("SPAM".into(), "SPAM".into(), "system".into()),
            ("STARRED".into(), "STARRED".into(), "system".into()),
            ("UNREAD".into(), "UNREAD".into(), "system".into()),
            ("Label_1".into(), "Clients".into(), "user".into()),
        ],
        msgs: vec![
            ("m1".into(), "t1".into(), vec!["INBOX".into(), "UNREAD".into(), "Label_1".into()], "First".into()),
            ("m2".into(), "t2".into(), vec!["INBOX".into()], "Second".into()),
        ],
        ..Default::default()
    }));
    let b = "/gmail/v1/users/me";

    let s = st.clone();
    Mock::given(method("GET")).and(path(format!("{b}/labels"))).respond_with(move |_: &Request| {
        let st = s.lock();
        ResponseTemplate::new(200).set_body_json(json!({ "labels": st.labels.iter().map(|(i, n, t)| json!({ "id": i, "name": n, "type": t })).collect::<Vec<_>>() }))
    }).mount(&server).await;
    let s = st.clone();
    Mock::given(method("POST")).and(path(format!("{b}/labels"))).respond_with(move |r: &Request| {
        let body: Value = serde_json::from_slice(&r.body).unwrap();
        let mut st = s.lock();
        let id = format!("Label_{}", 10 + st.labels.len());
        st.labels.push((id.clone(), body["name"].as_str().unwrap().into(), "user".into()));
        ResponseTemplate::new(200).set_body_json(json!({ "id": id, "name": body["name"] }))
    }).mount(&server).await;
    let s = st.clone();
    Mock::given(method("PATCH")).and(path_regex(format!("^{b}/labels/[^/]+$"))).respond_with(move |r: &Request| {
        let id = r.url.path().rsplit('/').next().unwrap().to_string();
        let body: Value = serde_json::from_slice(&r.body).unwrap();
        let mut st = s.lock();
        if let Some(l) = st.labels.iter_mut().find(|l| l.0 == id) {
            l.1 = body["name"].as_str().unwrap().into();
        }
        ResponseTemplate::new(200).set_body_json(json!({ "id": id }))
    }).mount(&server).await;
    let s = st.clone();
    Mock::given(method("DELETE")).and(path_regex(format!("^{b}/labels/[^/]+$"))).respond_with(move |r: &Request| {
        let id = r.url.path().rsplit('/').next().unwrap().to_string();
        s.lock().labels.retain(|l| l.0 != id);
        ResponseTemplate::new(204)
    }).mount(&server).await;
    Mock::given(method("GET")).and(path(format!("{b}/profile"))).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "emailAddress": "me@gmail.com", "historyId": "100" }))).mount(&server).await;
    let s = st.clone();
    Mock::given(method("GET")).and(path(format!("{b}/history"))).respond_with(move |r: &Request| {
        assert!(r.url.query_pairs().any(|(k, _)| k == "startHistoryId"));
        let mut st = s.lock();
        let added: Vec<Value> = st.history_added.drain(..).map(|id| json!({ "message": { "id": id } })).collect();
        ResponseTemplate::new(200).set_body_json(json!({ "history": [{ "id": "101", "messagesAdded": added }], "historyId": "101" }))
    }).mount(&server).await;
    let s = st.clone();
    Mock::given(method("GET")).and(path(format!("{b}/messages"))).respond_with(move |r: &Request| {
        assert!(r.url.query_pairs().any(|(k, v)| k == "includeSpamTrash" && v == "true"));
        let st = s.lock();
        ResponseTemplate::new(200).set_body_json(json!({ "messages": st.msgs.iter().map(|m| json!({ "id": m.0, "threadId": m.1 })).collect::<Vec<_>>() }))
    }).mount(&server).await;
    let s = st.clone();
    Mock::given(method("GET")).and(path_regex(format!("^{b}/messages/[^/]+$"))).respond_with(move |r: &Request| {
        let id = r.url.path().rsplit('/').next().unwrap().to_string();
        let format = r.url.query_pairs().find(|(k, _)| k == "format").map(|(_, v)| v.to_string()).unwrap_or_default();
        match gmail_msg_json(&s.lock(), &id, &format) {
            Some(v) => ResponseTemplate::new(200).set_body_json(v),
            None => ResponseTemplate::new(404).set_body_json(json!({ "error": { "code": 404, "message": "Not Found" } })),
        }
    }).mount(&server).await;
    let s = st.clone();
    Mock::given(method("POST")).and(path_regex(format!("^{b}/messages/[^/]+/modify$"))).respond_with(move |r: &Request| {
        let id = r.url.path().split('/').rev().nth(1).unwrap().to_string();
        let body: Value = serde_json::from_slice(&r.body).unwrap();
        let mut st = s.lock();
        let m = st.msgs.iter_mut().find(|m| m.0 == id).unwrap();
        for l in body["removeLabelIds"].as_array().unwrap() {
            m.2.retain(|x| x != l.as_str().unwrap());
        }
        for l in body["addLabelIds"].as_array().unwrap() {
            m.2.push(l.as_str().unwrap().into());
        }
        ResponseTemplate::new(200).set_body_json(json!({ "id": id, "labelIds": m.2 }))
    }).mount(&server).await;
    let s = st.clone();
    // users.messages.batchModify: same label change on up to 1000 ids; Google answers 204 with no body.
    Mock::given(method("POST")).and(path(format!("{b}/messages/batchModify"))).respond_with(move |r: &Request| {
        let body: Value = serde_json::from_slice(&r.body).unwrap();
        let mut st = s.lock();
        for id in body["ids"].as_array().unwrap() {
            let m = st.msgs.iter_mut().find(|m| m.0 == id.as_str().unwrap()).unwrap();
            for l in body["removeLabelIds"].as_array().unwrap() {
                m.2.retain(|x| x != l.as_str().unwrap());
            }
            for l in body["addLabelIds"].as_array().unwrap() {
                if !m.2.iter().any(|x| x == l.as_str().unwrap()) {
                    m.2.push(l.as_str().unwrap().into());
                }
            }
        }
        ResponseTemplate::new(204)
    }).mount(&server).await;
    let s = st.clone();
    Mock::given(method("POST")).and(path_regex(format!("^{b}/messages/[^/]+/(un)?trash$"))).respond_with(move |r: &Request| {
        let segs: Vec<&str> = r.url.path().rsplit('/').collect();
        let (action, id) = (segs[0], segs[1].to_string());
        let mut st = s.lock();
        let m = st.msgs.iter_mut().find(|m| m.0 == id).unwrap();
        if action == "trash" {
            m.2.retain(|x| x != "INBOX");
            m.2.push("TRASH".into());
        } else {
            m.2.retain(|x| x != "TRASH");
            m.2.push("INBOX".into());
        }
        ResponseTemplate::new(200).set_body_json(json!({ "id": id, "labelIds": m.2 }))
    }).mount(&server).await;
    Mock::given(method("POST")).and(path(format!("{b}/messages/send"))).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "sent1", "threadId": "t9" }))).mount(&server).await;
    let s = st.clone();
    Mock::given(method("POST")).and(path(format!("{b}/drafts"))).respond_with(move |_: &Request| {
        s.lock().drafts.insert("d1".into());
        ResponseTemplate::new(200).set_body_json(json!({ "id": "d1", "message": { "id": "md1", "threadId": "t8" } }))
    }).mount(&server).await;
    Mock::given(method("PUT")).and(path(format!("{b}/drafts/d1"))).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "d1", "message": { "id": "md2", "threadId": "t8" } }))).mount(&server).await;
    let s = st.clone();
    Mock::given(method("DELETE")).and(path(format!("{b}/drafts/d1"))).respond_with(move |_: &Request| {
        s.lock().drafts.remove("d1");
        ResponseTemplate::new(204)
    }).mount(&server).await;

    let (engine, events, secrets, _dir) = engine(&server).await;
    let email = "me@gmail.com";
    oauth::save_token(secrets.as_ref(), email, OAuthProvider::Google, "refresh", "access-token", far_future()).unwrap();
    let srv = |host: &str, port| ServerConfig { host: host.into(), port, security: Security::Tls, username: email.into(), auth: AuthKind::OauthGoogle };

    // account_test goes through the API (users.getProfile), not IMAP.
    let t = engine.account_test(srv("imap.gmail.com", 993), srv("smtp.gmail.com", 465), None).await.unwrap();
    assert!(t.imap_ok && t.smtp_ok, "{t:?}");

    let acct = engine
        .account_add(NewAccount { name: "Gmail".into(), email: email.into(), display_name: "Me".into(), provider: Provider::Gmail, imap: srv("imap.gmail.com", 993), smtp: srv("smtp.gmail.com", 465), password: None, color: None })
        .await
        .unwrap();

    // Initial sync: both messages, one row each, labels as folders.
    let page = wait_for("gmail initial sync", 20, || async { let p = engine.messages_list(q_inbox()).await.unwrap(); (p.total == 2).then_some(p) }).await;
    let folders = engine.folders_list(Some(acct.id.clone())).await.unwrap();
    let f = |name: &str| folders.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no folder {name}: {folders:?}")).clone();
    assert_eq!(f("Inbox").role, Some(FolderRole::Inbox));
    assert_eq!(f("Spam").role, Some(FolderRole::Junk));
    assert_eq!((f("Clients").total, f("Clients").unread), (1, 1));
    assert_eq!(f("All Mail").total, 2);
    let m1 = page.items.iter().find(|m| m.subject == "First").unwrap().clone();
    let m2 = page.items.iter().find(|m| m.subject == "Second").unwrap().clone();
    assert_eq!(m1.thread_id, "gt:t1");

    // Reading fetches format=raw and fills the headers the metadata list lacked.
    let full = engine.message_get(m1.id.clone()).await.unwrap();
    assert!(full.text.as_deref().unwrap_or_default().contains("Hello from First"), "{full:?}");
    assert!(full.list_unsubscribe.as_ref().is_some_and(|l| l.one_click));

    // Mark read → modify removes UNREAD.
    engine.messages_set_flags(vec![m1.id.clone()], vec!["seen".into()], vec![]).await.unwrap();
    assert!(!st.lock().msgs.iter().find(|m| m.0 == "m1").unwrap().2.contains(&"UNREAD".to_string()));
    assert_eq!(engine.folders_list(Some(acct.id.clone())).await.unwrap().iter().find(|x| x.name == "Clients").unwrap().unread, 0);

    // Archive → INBOX removed; undo puts it back.
    engine.messages_archive(vec![m1.id.clone()]).await.unwrap();
    assert_eq!(engine.messages_list(q_inbox()).await.unwrap().total, 1);
    assert_eq!(engine.messages_list(q_folder(&f("All Mail").id)).await.unwrap().total, 2);
    let token = events.find("undoable").last().unwrap()["token"].as_str().unwrap().to_string();
    engine.messages_undo(token).await.unwrap();
    assert!(st.lock().msgs.iter().find(|m| m.0 == "m1").unwrap().2.contains(&"INBOX".to_string()));
    assert_eq!(engine.messages_list(q_inbox()).await.unwrap().total, 2);

    // A tag becomes a Gmail user label of the same name.
    engine.messages_set_tags(vec![m2.id.clone()], vec!["$label1".into()], vec![]).await.unwrap();
    {
        let st = st.lock();
        let important = st.labels.iter().find(|l| l.1 == "Important").expect("label created for the tag").0.clone();
        assert!(st.msgs.iter().find(|m| m.0 == "m2").unwrap().2.contains(&important));
    }
    let m2_now = engine.messages_list(q_inbox()).await.unwrap().items.into_iter().find(|m| m.id == m2.id).unwrap();
    assert_eq!(m2_now.tags, vec!["$label1".to_string()]);

    // Trash → messages.trash; deleting from Trash is refused (gmail.modify cannot purge) with the reason.
    engine.messages_trash(vec![m2.id.clone()]).await.unwrap();
    assert!(st.lock().msgs.iter().find(|m| m.0 == "m2").unwrap().2.contains(&"TRASH".to_string()));
    assert_eq!(engine.messages_list(q_inbox()).await.unwrap().total, 1);
    let e = engine.messages_trash(vec![m2.id.clone()]).await.unwrap_err();
    assert!(e.message.contains("Trash"), "{e:?}");

    // New mail arrives through history.list on the next pass.
    {
        let mut st = st.lock();
        st.msgs.push(("m3".into(), "t3".into(), vec!["INBOX".into(), "UNREAD".into()], "Third".into()));
        st.history_added.push("m3".into());
    }
    engine.sync_now(Some(acct.id.clone())).await.unwrap();
    wait_for("history add", 20, || async { engine.messages_list(q_inbox()).await.unwrap().items.iter().any(|m| m.subject == "Third").then_some(()) }).await;
    wait_for("mail://new", 5, || async { events.find("mail://new").iter().any(|e| e["messages"][0]["subject"] == "Third").then_some(()) }).await;

    // Send → messages.send with base64url MIME that still carries Bcc (Gmail takes recipients from headers).
    let ident = acct.identities[0].id.clone();
    engine.send(draft(&ident, "bob@example.com", "Hello Bob")).await.unwrap();
    let sent = wait_for("gmail send", 20, || async {
        server.received_requests().await.unwrap().into_iter().find(|r| r.url.path() == format!("{b}/messages/send"))
    })
    .await;
    let body: Value = serde_json::from_slice(&sent.body).unwrap();
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(body["raw"].as_str().unwrap()).unwrap();
    let raw = String::from_utf8_lossy(&raw);
    assert!(raw.contains("Subject: Hello Bob") && raw.contains("secret@example.com"), "{raw}");
    assert!(!sent.headers.get("authorization").unwrap().to_str().unwrap().is_empty());

    // Drafts: create, replace (PUT same draft), delete.
    let saved = engine.draft_save(draft(&ident, "carol@example.com", "Draft one")).await.unwrap();
    assert!(saved.server_saved, "{saved:?}");
    let mut d2 = draft(&ident, "carol@example.com", "Draft two");
    d2.id = Some(saved.id.clone());
    assert!(engine.draft_save(d2).await.unwrap().server_saved);
    assert!(server.received_requests().await.unwrap().iter().any(|r| r.method.as_str() == "PUT" && r.url.path() == format!("{b}/drafts/d1")));
    engine.draft_delete(saved.id.clone()).await.unwrap();
    assert!(st.lock().drafts.is_empty());

    // Folders = labels: create, rename, delete.
    let projects = engine.folder_create(acct.id.clone(), None, "Projects".into()).await.unwrap();
    assert!(st.lock().labels.iter().any(|l| l.1 == "Projects"));
    let renamed = engine.folder_rename(projects.id.clone(), "Jobs".into()).await.unwrap();
    assert_eq!(renamed.name, "Jobs");
    assert!(st.lock().labels.iter().any(|l| l.1 == "Jobs"));
    engine.folder_delete(renamed.id.clone()).await.unwrap();
    assert!(!st.lock().labels.iter().any(|l| l.1 == "Jobs"));
    assert!(engine.folders_list(Some(acct.id.clone())).await.unwrap().iter().all(|x| x.name != "Jobs"));

    // Every request carried the bearer token.
    for r in server.received_requests().await.unwrap() {
        assert_eq!(r.headers.get("authorization").unwrap().to_str().unwrap(), "Bearer access-token");
    }
}

// ------------------------------------------------------------------------------------------------ Graph

#[derive(Default)]
struct GraphState {
    inbox_round_done: bool,
    pending_inbox: Vec<Value>,
    patched: Vec<(String, Value)>,
    moves: Vec<(String, String)>,
    purged: Vec<String>,
    drafts: BTreeSet<String>,
    folders_created: Vec<String>,
}

fn gmsg(id: &str, subject: &str, read: bool) -> Value {
    json!({ "id": id, "subject": subject, "from": { "emailAddress": { "name": "Dan", "address": "dan@example.com" } },
            "toRecipients": [{ "emailAddress": { "name": "Me", "address": "me@contoso.com" } }],
            "receivedDateTime": "2026-10-27T10:00:00Z", "sentDateTime": "2026-10-27T09:59:00Z",
            "internetMessageId": format!("<{id}@contoso.com>"), "conversationId": format!("C-{id}"),
            "isRead": read, "isDraft": false, "flag": { "flagStatus": "notFlagged" }, "categories": [],
            "hasAttachments": false, "bodyPreview": format!("preview {subject}") })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graph_account_end_to_end() {
    let server = MockServer::start().await;
    let st = Arc::new(Mutex::new(GraphState::default()));
    let b = "/graph/me";
    let uri = server.uri();

    for (wk, id) in [("inbox", "F-IN"), ("sentitems", "F-SE"), ("drafts", "F-DR"), ("deleteditems", "F-DE"), ("junkemail", "F-JU"), ("archive", "F-AR")] {
        Mock::given(method("GET")).and(path(format!("{b}/mailFolders/{wk}"))).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": id }))).mount(&server).await;
    }
    Mock::given(method("GET")).and(path(format!("{b}/mailFolders/outbox"))).respond_with(ResponseTemplate::new(404).set_body_json(json!({ "error": { "code": "ErrorItemNotFound", "message": "no" } }))).mount(&server).await;
    let s = st.clone();
    Mock::given(method("GET")).and(path(format!("{b}/mailFolders"))).respond_with(move |_: &Request| {
        let mut v = vec![
            json!({ "id": "F-IN", "displayName": "Inbox", "childFolderCount": 1 }),
            json!({ "id": "F-SE", "displayName": "Sent Items", "childFolderCount": 0 }),
            json!({ "id": "F-DR", "displayName": "Drafts", "childFolderCount": 0 }),
            json!({ "id": "F-DE", "displayName": "Deleted Items", "childFolderCount": 0 }),
            json!({ "id": "F-JU", "displayName": "Junk Email", "childFolderCount": 0 }),
            json!({ "id": "F-AR", "displayName": "Archive", "childFolderCount": 0 }),
            json!({ "@odata.type": "#microsoft.graph.mailSearchFolder", "id": "F-SR", "displayName": "Weekly digests", "childFolderCount": 0 }),
        ];
        for (i, n) in s.lock().folders_created.iter().enumerate() {
            v.push(json!({ "id": format!("F-NEW{i}"), "displayName": n, "childFolderCount": 0 }));
        }
        ResponseTemplate::new(200).set_body_json(json!({ "value": v }))
    }).mount(&server).await;
    Mock::given(method("GET")).and(path(format!("{b}/mailFolders/F-IN/childFolders"))).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": [{ "id": "F-PR", "displayName": "Projects", "childFolderCount": 0 }] }))).mount(&server).await;

    // Inbox delta: a two-page first round (nextLink, then deltaLink), then incremental rounds.
    let (s, u) = (st.clone(), uri.clone());
    Mock::given(method("GET")).and(path(format!("{b}/mailFolders/F-IN/messages/delta"))).respond_with(move |r: &Request| {
        let q: Vec<(String, String)> = r.url.query_pairs().into_owned().collect();
        let get = |k: &str| q.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        let next = format!("{u}/graph/me/mailFolders/F-IN/messages/delta?$skiptoken=2");
        let delta = format!("{u}/graph/me/mailFolders/F-IN/messages/delta?$deltatoken=d1");
        if get("$skiptoken").is_some() {
            s.lock().inbox_round_done = true;
            return ResponseTemplate::new(200).set_body_json(json!({ "value": [gmsg("A2", "Second", true)], "@odata.deltaLink": delta }));
        }
        if get("$deltatoken").is_some() {
            let items: Vec<Value> = std::mem::take(&mut s.lock().pending_inbox);
            return ResponseTemplate::new(200).set_body_json(json!({ "value": items, "@odata.deltaLink": delta }));
        }
        assert!(get("$select").is_some_and(|v| v.contains("conversationId")), "initial delta selects fields");
        ResponseTemplate::new(200).set_body_json(json!({ "value": [gmsg("A1", "First", false)], "@odata.nextLink": next }))
    }).mount(&server).await;
    let u = uri.clone();
    Mock::given(method("GET")).and(path_regex(format!("^{b}/mailFolders/F-[A-Z0-9]+/messages/delta$"))).respond_with(move |r: &Request| {
        let fid = r.url.path().split('/').nth(4).unwrap().to_string();
        ResponseTemplate::new(200).set_body_json(json!({ "value": [], "@odata.deltaLink": format!("{u}/graph/me/mailFolders/{fid}/messages/delta?$deltatoken=x") }))
    }).mount(&server).await;
    Mock::given(method("GET")).and(path_regex(format!(r"^{b}/messages/[^/]+/\$value$"))).respond_with(|r: &Request| {
        let id = r.url.path().split('/').nth(4).unwrap().to_string();
        ResponseTemplate::new(200).set_body_string(raw_mime(&format!("Graph {id}"), &format!("{id}@contoso.com")))
    }).mount(&server).await;
    let s = st.clone();
    Mock::given(method("PATCH")).and(path_regex(format!("^{b}/messages/[^/]+$"))).respond_with(move |r: &Request| {
        let id = r.url.path().rsplit('/').next().unwrap().to_string();
        s.lock().patched.push((id.clone(), serde_json::from_slice(&r.body).unwrap()));
        ResponseTemplate::new(200).set_body_json(json!({ "id": id }))
    }).mount(&server).await;
    let s = st.clone();
    Mock::given(method("POST")).and(path_regex(format!("^{b}/messages/[^/]+/move$"))).respond_with(move |r: &Request| {
        let id = r.url.path().split('/').nth(4).unwrap().to_string();
        let body: Value = serde_json::from_slice(&r.body).unwrap();
        s.lock().moves.push((id.clone(), body["destinationId"].as_str().unwrap().to_string()));
        ResponseTemplate::new(201).set_body_json(json!({ "id": id }))
    }).mount(&server).await;
    let s = st.clone();
    Mock::given(method("POST")).and(path_regex(format!("^{b}/messages/[^/]+/permanentDelete$"))).respond_with(move |r: &Request| {
        s.lock().purged.push(r.url.path().split('/').nth(4).unwrap().to_string());
        ResponseTemplate::new(204)
    }).mount(&server).await;
    Mock::given(method("POST")).and(path(format!("{b}/sendMail"))).respond_with(ResponseTemplate::new(202)).mount(&server).await;
    let s = st.clone();
    Mock::given(method("POST")).and(path(format!("{b}/messages"))).respond_with(move |_: &Request| {
        let mut st = s.lock();
        let id = format!("DR{}", st.drafts.len() + 1);
        st.drafts.insert(id.clone());
        ResponseTemplate::new(201).set_body_json(json!({ "id": id, "isDraft": true }))
    }).mount(&server).await;
    let s = st.clone();
    Mock::given(method("DELETE")).and(path_regex(format!("^{b}/messages/[^/]+$"))).respond_with(move |r: &Request| {
        s.lock().drafts.remove(r.url.path().rsplit('/').next().unwrap());
        ResponseTemplate::new(204)
    }).mount(&server).await;
    let s = st.clone();
    Mock::given(method("POST")).and(path(format!("{b}/mailFolders"))).respond_with(move |r: &Request| {
        let body: Value = serde_json::from_slice(&r.body).unwrap();
        let mut st = s.lock();
        let i = st.folders_created.len();
        st.folders_created.push(body["displayName"].as_str().unwrap().into());
        ResponseTemplate::new(201).set_body_json(json!({ "id": format!("F-NEW{i}") }))
    }).mount(&server).await;
    Mock::given(method("GET")).and(path(format!("{b}/mailFolders/inbox"))).and(query_param("$select", "id")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "F-IN" }))).mount(&server).await;

    let (engine, events, secrets, _dir) = engine(&server).await;
    let email = "me@contoso.com";
    oauth::save_token(secrets.as_ref(), email, OAuthProvider::Microsoft, "refresh", "graph-token", far_future()).unwrap();
    let srv = |host: &str, port| ServerConfig { host: host.into(), port, security: Security::Tls, username: email.into(), auth: AuthKind::OauthMicrosoft };
    let t = engine.account_test(srv("outlook.office365.com", 993), srv("smtp-mail.outlook.com", 587), None).await.unwrap();
    assert!(t.imap_ok && t.smtp_ok, "{t:?}");
    let acct = engine
        .account_add(NewAccount { name: "Work".into(), email: email.into(), display_name: "Me".into(), provider: Provider::Outlook, imap: srv("outlook.office365.com", 993), smtp: srv("smtp-mail.outlook.com", 587), password: None, color: None })
        .await
        .unwrap();

    let page = wait_for("graph first round", 20, || async { let p = engine.messages_list(q_inbox()).await.unwrap(); (p.total == 2).then_some(p) }).await;
    assert!(st.lock().inbox_round_done);
    let folders = engine.folders_list(Some(acct.id.clone())).await.unwrap();
    let f = |name: &str| folders.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no folder {name}: {folders:?}")).clone();
    assert_eq!(f("Deleted Items").role, Some(FolderRole::Trash));
    assert_eq!(f("Archive").role, Some(FolderRole::Archive));
    assert_eq!(f("Projects").parent_id.as_deref(), Some(f("Inbox").id.as_str()));
    assert!(folders.iter().all(|x| x.name != "Weekly digests"), "search folders are not synced");
    let a1 = page.items.iter().find(|m| m.subject == "First").unwrap().clone();
    let a2 = page.items.iter().find(|m| m.subject == "Second").unwrap().clone();
    assert_eq!(a1.thread_id, "gc:C-A1");
    assert!(!a1.flags.seen);

    let full = engine.message_get(a1.id.clone()).await.unwrap();
    assert!(full.text.as_deref().unwrap_or_default().contains("Hello from Graph A1"));
    assert!(full.list_unsubscribe.is_some(), "headers absent from the Graph list are filled from MIME");

    // Read + flag → PATCH isRead / flag.
    engine.messages_set_flags(vec![a1.id.clone()], vec!["seen".into(), "flagged".into()], vec![]).await.unwrap();
    {
        let st = st.lock();
        let p = &st.patched.iter().find(|(id, _)| id == "A1").unwrap().1;
        assert_eq!(p["isRead"], true);
        assert_eq!(p["flag"]["flagStatus"], "flagged");
    }

    // Tags → categories (names).
    engine.messages_set_tags(vec![a1.id.clone()], vec!["$label2".into()], vec![]).await.unwrap();
    assert!(st.lock().patched.iter().any(|(id, b)| id == "A1" && b["categories"] == json!(["Work"])));

    // Archive → move to "archive"; undo moves it back to the Inbox folder id.
    engine.messages_archive(vec![a2.id.clone()]).await.unwrap();
    assert_eq!(st.lock().moves.last().unwrap(), &("A2".to_string(), "archive".to_string()));
    assert_eq!(engine.messages_list(q_folder(&f("Archive").id)).await.unwrap().total, 1);
    let token = events.find("undoable").last().unwrap()["token"].as_str().unwrap().to_string();
    engine.messages_undo(token).await.unwrap();
    assert_eq!(st.lock().moves.last().unwrap(), &("A2".to_string(), "F-IN".to_string()));
    assert_eq!(engine.messages_list(q_inbox()).await.unwrap().total, 2);

    // Trash → move to deleteditems; again from Trash → permanentDelete.
    engine.messages_trash(vec![a2.id.clone()]).await.unwrap();
    assert_eq!(st.lock().moves.last().unwrap(), &("A2".to_string(), "deleteditems".to_string()));
    engine.messages_trash(vec![a2.id.clone()]).await.unwrap();
    assert_eq!(st.lock().purged, vec!["A2".to_string()]);

    // Incremental: a new message and a removal in the next delta round.
    st.lock().pending_inbox = vec![gmsg("A3", "Third", false), json!({ "id": "A1", "@removed": { "reason": "deleted" } })];
    engine.sync_now(Some(acct.id.clone())).await.unwrap();
    wait_for("graph delta", 20, || async {
        let items = engine.messages_list(q_inbox()).await.unwrap().items;
        (items.iter().any(|m| m.subject == "Third") && !items.iter().any(|m| m.subject == "First")).then_some(())
    })
    .await;

    // Send → sendMail with standard-base64 MIME, Bcc kept.
    let ident = acct.identities[0].id.clone();
    engine.send(draft(&ident, "eve@example.com", "Hi Eve")).await.unwrap();
    let sent = wait_for("graph send", 20, || async { server.received_requests().await.unwrap().into_iter().find(|r| r.url.path() == format!("{b}/sendMail")) }).await;
    assert_eq!(sent.headers.get("content-type").unwrap().to_str().unwrap(), "text/plain");
    let mime = base64::engine::general_purpose::STANDARD.decode(&sent.body).unwrap();
    let mime = String::from_utf8_lossy(&mime);
    assert!(mime.contains("Subject: Hi Eve") && mime.contains("secret@example.com"), "{mime}");

    // Drafts: create, replace (new + delete old), discard.
    let saved = engine.draft_save(draft(&ident, "f@example.com", "D1")).await.unwrap();
    assert!(saved.server_saved, "{saved:?}");
    let mut d2 = draft(&ident, "f@example.com", "D2");
    d2.id = Some(saved.id.clone());
    engine.draft_save(d2).await.unwrap();
    assert_eq!(st.lock().drafts.iter().cloned().collect::<Vec<_>>(), vec!["DR2".to_string()], "the old draft version is deleted");
    engine.draft_delete(saved.id.clone()).await.unwrap();
    assert!(st.lock().drafts.is_empty());

    // Folder create at the top level.
    let nf = engine.folder_create(acct.id.clone(), None, "Clients".into()).await.unwrap();
    assert_eq!(nf.name, "Clients");
    assert_eq!(st.lock().folders_created, vec!["Clients".to_string()]);

    // Every Graph request asked for immutable ids and carried the token.
    for r in server.received_requests().await.unwrap() {
        let prefer: Vec<String> = r.headers.get_all("prefer").iter().map(|v| v.to_str().unwrap().to_string()).collect();
        assert!(prefer.iter().any(|p| p == "IdType=\"ImmutableId\""), "{} {:?}", r.url, prefer);
        assert_eq!(r.headers.get("authorization").unwrap().to_str().unwrap(), "Bearer graph-token");
    }
}

// ------------------------------------------------------------------------------------------------ throttling

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_after_is_honoured() {
    let server = MockServer::start().await;
    let hits = Arc::new(Mutex::new(0u32));
    let h = hits.clone();
    Mock::given(method("GET")).and(path("/graph/me/mailFolders/inbox")).respond_with(move |_: &Request| {
        let mut n = h.lock();
        *n += 1;
        if *n == 1 {
            ResponseTemplate::new(429).insert_header("Retry-After", "1").set_body_json(json!({ "error": { "code": "TooManyRequests", "message": "slow down" } }))
        } else {
            ResponseTemplate::new(200).set_body_json(json!({ "id": "F-IN" }))
        }
    }).mount(&server).await;
    let (engine, _events, secrets, _dir) = engine(&server).await;
    oauth::save_token(secrets.as_ref(), "x@contoso.com", OAuthProvider::Microsoft, "r", "t", far_future()).unwrap();
    let cfg = ServerConfig { host: "outlook.office365.com".into(), port: 993, security: Security::Tls, username: "x@contoso.com".into(), auth: AuthKind::OauthMicrosoft };
    let started = std::time::Instant::now();
    let t = engine.account_test(cfg.clone(), cfg, None).await.unwrap();
    assert!(t.imap_ok, "{t:?}");
    assert_eq!(*hits.lock(), 2);
    assert!(started.elapsed() >= Duration::from_millis(900), "waited for Retry-After");
}
