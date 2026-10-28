//! Conversation threading.
//!
//! Source: Jamie Zawinski, "Message Threading" (https://www.jwz.org/doc/threading.html) — threads are built from
//! Message-ID / In-Reply-To / References; a referenced message that has not arrived yet still gets a container
//! (here: a `thread_ids` row), so children that arrive before their parent still join one thread. Subject
//! grouping is only used when a message has NO references and its subject is a reply ("Re: …"), as JWZ's step 5.
//! Gmail's X-GM-THRID, when present, is authoritative for that account.
use crate::error::Result;
use crate::mime::{base_subject, is_reply_subject};
use rusqlite::{params, Connection, OptionalExtension};

/// Window for the subject fallback (JWZ leaves this to the implementation; Thunderbird uses a similar recency bound).
const SUBJECT_WINDOW_MS: i64 = 60 * 24 * 3600 * 1000;

pub struct ThreadInput<'a> {
    pub account_id: &'a str,
    pub message_id: Option<&'a str>,
    pub in_reply_to: Option<&'a str>,
    pub references: &'a [String],
    pub subject: &'a str,
    pub date: i64,
    pub gm_thrid: Option<u64>,
    /// Thread id assigned by an API provider (Gmail `threadId` → "gt:…", Graph `conversationId` → "gc:…").
    /// Like X-GM-THRID it is authoritative for that account.
    pub ext_thread: Option<&'a str>,
}

pub fn assign_thread(conn: &Connection, t: &ThreadInput<'_>) -> Result<String> {
    let mut ids: Vec<String> = t.references.to_vec();
    if let Some(irt) = t.in_reply_to {
        if !ids.iter().any(|x| x == irt) {
            ids.push(irt.to_string());
        }
    }
    let has_refs = !ids.is_empty();
    if let Some(mid) = t.message_id {
        ids.push(mid.to_string());
    }

    if let Some(tid) = t.ext_thread.filter(|s| !s.is_empty()) {
        register(conn, t.account_id, &ids, tid)?;
        return Ok(tid.to_string());
    }

    if let Some(thrid) = t.gm_thrid {
        let tid = format!("gm:{thrid}");
        register(conn, t.account_id, &ids, &tid)?;
        return Ok(tid);
    }

    let mut found: Vec<String> = Vec::new();
    {
        let mut stmt = conn.prepare_cached("SELECT thread_id FROM thread_ids WHERE account_id=?1 AND message_id=?2")?;
        for id in &ids {
            if let Some(tid) = stmt.query_row(params![t.account_id, id], |r| r.get::<_, String>(0)).optional()? {
                if !found.contains(&tid) {
                    found.push(tid);
                }
            }
        }
    }

    let tid = if found.is_empty() {
        let mut tid = None;
        if !has_refs && is_reply_subject(t.subject) {
            let base = base_subject(t.subject);
            if !base.is_empty() {
                tid = conn
                    .query_row(
                        "SELECT thread_id FROM messages WHERE account_id=?1 AND base_subject=?2 AND date BETWEEN ?3 AND ?4 ORDER BY date DESC LIMIT 1",
                        params![t.account_id, base, t.date - SUBJECT_WINDOW_MS, t.date + SUBJECT_WINDOW_MS],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?;
            }
        }
        tid.unwrap_or_else(|| match t.message_id {
            Some(mid) => format!("m:{mid}"),
            None => format!("u:{}", uuid::Uuid::new_v4()),
        })
    } else {
        found.sort();
        let keep = found[0].clone();
        for other in &found[1..] {
            conn.execute("UPDATE messages SET thread_id=?1 WHERE account_id=?2 AND thread_id=?3", params![keep, t.account_id, other])?;
            conn.execute("UPDATE thread_ids SET thread_id=?1 WHERE account_id=?2 AND thread_id=?3", params![keep, t.account_id, other])?;
        }
        keep
    };
    register(conn, t.account_id, &ids, &tid)?;
    Ok(tid)
}

fn register(conn: &Connection, account_id: &str, ids: &[String], tid: &str) -> Result<()> {
    let mut stmt = conn.prepare_cached("INSERT OR REPLACE INTO thread_ids(account_id, message_id, thread_id) VALUES (?1, ?2, ?3)")?;
    for id in ids {
        stmt.execute(params![account_id, id, tid])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Store;

    fn input<'a>(mid: &'a str, irt: Option<&'a str>, refs: &'a [String], subject: &'a str) -> ThreadInput<'a> {
        ThreadInput { account_id: "a", message_id: Some(mid), in_reply_to: irt, references: refs, subject, date: 1_000_000, gm_thrid: None, ext_thread: None }
    }

    #[test]
    fn parent_child_and_out_of_order() {
        let s = Store::open_in_memory().unwrap();
        s.with(|c| {
            // Child arrives before parent.
            let refs = vec!["root@x".to_string()];
            let t_child = assign_thread(c, &input("child@x", Some("root@x"), &refs, "Re: Plan"))?;
            let t_root = assign_thread(c, &input("root@x", None, &[], "Plan"))?;
            assert_eq!(t_child, t_root);
            // Grandchild references only the child.
            let refs2 = vec!["child@x".to_string()];
            let t_g = assign_thread(c, &input("g@x", Some("child@x"), &refs2, "Re: Re: Plan"))?;
            assert_eq!(t_g, t_root);
            // Unrelated message.
            let t_other = assign_thread(c, &input("other@x", None, &[], "Lunch"))?;
            assert_ne!(t_other, t_root);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn merges_two_threads_when_a_message_links_them() {
        let s = Store::open_in_memory().unwrap();
        s.with(|c| {
            let a = assign_thread(c, &input("a@x", None, &[], "A"))?;
            let b = assign_thread(c, &input("b@x", None, &[], "B"))?;
            assert_ne!(a, b);
            let refs = vec!["a@x".to_string(), "b@x".to_string()];
            let m = assign_thread(c, &input("m@x", None, &refs, "Re: A"))?;
            let a2: String = c.query_row("SELECT thread_id FROM thread_ids WHERE message_id='a@x'", [], |r| r.get(0))?;
            let b2: String = c.query_row("SELECT thread_id FROM thread_ids WHERE message_id='b@x'", [], |r| r.get(0))?;
            assert_eq!(a2, m);
            assert_eq!(b2, m);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn gmail_thrid_wins() {
        let s = Store::open_in_memory().unwrap();
        s.with(|c| {
            let mut t = input("x@x", None, &[], "Hello");
            t.gm_thrid = Some(42);
            assert_eq!(assign_thread(c, &t)?, "gm:42");
            Ok(())
        })
        .unwrap();
    }
}
