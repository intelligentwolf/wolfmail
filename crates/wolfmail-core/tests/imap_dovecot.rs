//! Integration tests against a LOCAL Dovecot 2.3.21 (CONDSTORE, QRESYNC, IDLE, MOVE, UIDPLUS, SPECIAL-USE).
//! Start it with:
//!   docker run -d --rm --name wm-dovecot -p 127.0.0.1:31143:143 \
//!     -v "$PWD/tests/fixtures/dovecot-test.conf:/etc/dovecot/dovecot.conf:ro" dovecot/dovecot:2.3.21
//! and run `WOLFMAIL_IT_DOVECOT=127.0.0.1:31143 cargo test --test imap_dovecot`. Skipped when the variable is unset.
use std::time::Duration;
use wolfmail_core::db::Store;
use wolfmail_core::imap::conn::{connect, Conn, Credential};
use wolfmail_core::imap::sync::{sync_folder, sync_folder_list, AccountCtx};
use wolfmail_core::model::*;
use wolfmail_core::msgstore;

fn server() -> Option<(String, u16)> {
    let v = std::env::var("WOLFMAIL_IT_DOVECOT").ok()?;
    let (h, p) = v.rsplit_once(':')?;
    Some((h.to_string(), p.parse().ok()?))
}

fn cfg(host: &str, port: u16, user: &str) -> ServerConfig {
    ServerConfig { host: host.into(), port, security: Security::None, username: user.into(), auth: AuthKind::Password }
}

async fn login(host: &str, port: u16, user: &str) -> Conn {
    connect(&cfg(host, port, user), &Credential::Password { user: user.into(), pass: "pass".into() }).await.expect("login")
}

fn msg(n: u32, mid: &str, irt: Option<&str>, subject: &str) -> Vec<u8> {
    let refs = irt.map(|r| format!("In-Reply-To: <{r}>\r\nReferences: <{r}>\r\n")).unwrap_or_default();
    format!(
        "From: Tester {n} <t{n}@example.org>\r\nTo: me@example.test\r\nSubject: {subject}\r\nDate: Tue, 27 Oct 2026 1{n}:00:00 +0000\r\nMessage-ID: <{mid}>\r\n{refs}MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBody number {n}.\r\n"
    )
    .into_bytes()
}

fn seed_account(store: &Store, id: &str) {
    store
        .with(|c| {
            c.execute(
                "INSERT INTO accounts(id,name,email,color,provider,imap_json,smtp_json) VALUES (?1,'T','t@x','#c41e1e','imap','{}','{}')",
                [id],
            )?;
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn full_sync_qresync_flags_expunge_move_idle() {
    let Some((host, port)) = server() else {
        eprintln!("skipped: set WOLFMAIL_IT_DOVECOT");
        return;
    };
    let user = format!("it-{}", uuid::Uuid::new_v4().simple());
    let mut other = login(&host, port, &user).await;
    let mut conn = login(&host, port, &user).await;
    assert!(conn.qresync, "Dovecot advertises QRESYNC and ENABLE; the engine must switch it on");

    for (i, (mid, irt, subj)) in [("a@x", None, "Plan"), ("b@x", Some("a@x"), "Re: Plan"), ("c@x", None, "Lunch")].iter().enumerate() {
        other.append_msg("INBOX", "", &msg(i as u32 + 1, mid, *irt, subj)).await.unwrap();
    }

    let store = Store::open_in_memory().unwrap();
    seed_account(&store, "acc");
    let ctx = AccountCtx { account_id: "acc".into(), body_days: 30 };
    let folders = sync_folder_list(&mut conn, &store, "acc").await.unwrap();
    let roles: Vec<Option<FolderRole>> = folders.iter().map(|f| f.role).collect();
    for r in [FolderRole::Inbox, FolderRole::Sent, FolderRole::Drafts, FolderRole::Trash, FolderRole::Junk, FolderRole::Archive] {
        assert!(roles.contains(&Some(r)), "missing role {r:?}: {roles:?}");
    }
    let inbox = folders.iter().find(|f| f.role == Some(FolderRole::Inbox)).unwrap().clone();
    let archive = folders.iter().find(|f| f.role == Some(FolderRole::Archive)).unwrap().clone();

    // First sync: 3 messages, bodies prefetched, two threads.
    let out = sync_folder(&mut conn, &store, &ctx, &inbox).await.unwrap();
    assert!(out.changed);
    assert!(out.new_ids.is_empty(), "first sync is not 'new mail'");
    let q = ViewQuery { folder_id: Some(inbox.id.to_string()), unified: None, filter: None, search: None, threaded: true, sort: SortOrder::DateDesc, offset: 0, limit: 50 };
    let page = store.with(|c| msgstore::list_view(c, &q)).unwrap();
    assert_eq!(page.total, 2, "Plan + Re: Plan form one thread");
    let cached: i64 = store.with(|c| Ok(c.query_row("SELECT COUNT(*) FROM messages WHERE body_cached=1", [], |r| r.get(0))?)).unwrap();
    assert_eq!(cached, 3);
    let snippet: String = store.with(|c| Ok(c.query_row("SELECT snippet FROM messages WHERE message_id='c@x'", [], |r| r.get(0))?)).unwrap();
    assert_eq!(snippet, "Body number 3.");

    // Another client: mark one read, expunge one, add one.
    other.select("INBOX", None).await.unwrap();
    other.raw("UID STORE 1 +FLAGS (\\Seen)").await.unwrap();
    other.delete_uids("INBOX", &[3]).await.unwrap();
    other.append_msg("INBOX", "", &msg(4, "d@x", Some("b@x"), "Re: Re: Plan")).await.unwrap();

    // Second sync goes through SELECT … (QRESYNC (uidvalidity modseq)).
    let out = sync_folder(&mut conn, &store, &ctx, &inbox).await.unwrap();
    assert!(out.changed);
    assert_eq!(out.new_ids.len(), 1, "exactly the appended message is new");
    let (seen, total, gone): (bool, i64, i64) = store
        .with(|c| {
            Ok((
                c.query_row("SELECT seen FROM messages WHERE message_id='a@x'", [], |r| r.get(0))?,
                c.query_row("SELECT COUNT(*) FROM messages WHERE folder_id=?1", [inbox.id], |r| r.get(0))?,
                c.query_row("SELECT COUNT(*) FROM messages WHERE message_id='c@x'", [], |r| r.get(0))?,
            ))
        })
        .unwrap();
    assert!(seen, "flag change seen through QRESYNC");
    assert_eq!(total, 3);
    assert_eq!(gone, 0, "expunge seen through VANISHED");
    let t: Vec<String> = store
        .with(|c| {
            let mut st = c.prepare("SELECT DISTINCT thread_id FROM messages WHERE message_id IN ('a@x','b@x','d@x')")?;
            let v = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(v)
        })
        .unwrap();
    assert_eq!(t.len(), 1, "grandchild joins the thread");

    // Nothing changed → nothing changes.
    let out = sync_folder(&mut conn, &store, &ctx, &inbox).await.unwrap();
    assert!(!out.changed && out.new_ids.is_empty());

    // MOVE reports COPYUID (UIDPLUS).
    let mapping = conn.move_uids("INBOX", &[2], &archive.path).await.unwrap();
    let m = mapping.expect("COPYUID");
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].0, 2);

    // APPENDUID comes back from our APPEND.
    let uid = conn.append_msg(&archive.path, "\\Seen", &msg(5, "e@x", None, "Filed")).await.unwrap();
    assert!(uid.is_some());
    assert_eq!(conn.find_uid_by_message_id(&archive.path, "e@x").await.unwrap(), uid);

    // Keyword (tag) round trip.
    conn.store_flags("INBOX", &[1], &["$label1".into()], &[]).await.unwrap();
    let out = sync_folder(&mut conn, &store, &ctx, &inbox).await.unwrap();
    assert!(out.changed);
    let kw: String = store.with(|c| Ok(c.query_row("SELECT keywords FROM messages WHERE message_id='a@x'", [], |r| r.get(0))?)).unwrap();
    assert!(kw.contains("$label1"), "{kw}");

    // IDLE wakes on new mail.
    let mut idler = login(&host, port, &user).await;
    idler.select("INBOX", None).await.unwrap();
    let Conn { session, .. } = idler;
    let mut idle = session.idle();
    idle.init().await.unwrap();
    let appender = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        other.append_msg("INBOX", "", &msg(6, "f@x", None, "Ping")).await.unwrap();
        other
    });
    let (fut, _stop) = idle.wait_with_timeout(Duration::from_secs(10));
    let r = fut.await.unwrap();
    assert!(matches!(r, async_imap::extensions::idle::IdleResponse::NewData(_)), "{r:?}");
    let _ = appender.await.unwrap();

    // Folder management.
    conn.create_folder("Projects/Wolf").await.unwrap();
    let folders = sync_folder_list(&mut conn, &store, "acc").await.unwrap();
    assert!(folders.iter().any(|f| f.path == "Projects/Wolf"));
    conn.rename_folder("Projects/Wolf", "Projects/WolfMail").await.unwrap();
    conn.delete_folder("Projects/WolfMail").await.unwrap();
    let folders = sync_folder_list(&mut conn, &store, "acc").await.unwrap();
    assert!(!folders.iter().any(|f| f.path.starts_with("Projects/Wolf")));
}

#[tokio::test]
async fn uidvalidity_change_resets_folder() {
    let Some((host, port)) = server() else {
        eprintln!("skipped: set WOLFMAIL_IT_DOVECOT");
        return;
    };
    let user = format!("it-{}", uuid::Uuid::new_v4().simple());
    let mut conn = login(&host, port, &user).await;
    conn.create_folder("Box").await.unwrap();
    conn.append_msg("Box", "", &msg(1, "x1@x", None, "One")).await.unwrap();
    let store = Store::open_in_memory().unwrap();
    seed_account(&store, "acc");
    let ctx = AccountCtx { account_id: "acc".into(), body_days: 0 };
    let folders = sync_folder_list(&mut conn, &store, "acc").await.unwrap();
    let b = folders.iter().find(|f| f.path == "Box").unwrap().clone();
    sync_folder(&mut conn, &store, &ctx, &b).await.unwrap();
    // Recreate the mailbox → new UIDVALIDITY.
    conn.delete_folder("Box").await.unwrap();
    conn.create_folder("Box").await.unwrap();
    conn.append_msg("Box", "", &msg(2, "x2@x", None, "Two")).await.unwrap();
    let out = sync_folder(&mut conn, &store, &ctx, &b).await.unwrap();
    assert!(out.changed);
    let mids: Vec<String> = store
        .with(|c| {
            let mut st = c.prepare("SELECT message_id FROM messages WHERE folder_id=?1")?;
            let v = st.query_map([b.id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(v)
        })
        .unwrap();
    assert_eq!(mids, vec!["x2@x".to_string()]);
}
