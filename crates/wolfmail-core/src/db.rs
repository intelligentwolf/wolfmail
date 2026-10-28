//! Local store: one SQLite file per profile (WAL), FTS5 index over headers + cached body text.
//! Every call runs on the blocking pool; the connection lock is a parking_lot mutex that is never held across
//! an `.await` (the closure is synchronous by construction).
// Source: docs/API.md — the tables hold exactly what the contract's types need.
use crate::error::{ApiError, Result};
use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Arc;

const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE accounts (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        email TEXT NOT NULL,
        color TEXT NOT NULL,
        provider TEXT NOT NULL,
        imap_json TEXT NOT NULL,
        smtp_json TEXT NOT NULL,
        body_days INTEGER NOT NULL DEFAULT 30,
        enabled INTEGER NOT NULL DEFAULT 1,
        sort_order INTEGER NOT NULL DEFAULT 0,
        status TEXT NOT NULL DEFAULT 'ok',
        status_message TEXT,
        last_sync_at INTEGER,
        capabilities TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE identities (
        id TEXT PRIMARY KEY,
        account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        email TEXT NOT NULL,
        reply_to TEXT,
        signature_html TEXT NOT NULL DEFAULT '',
        reply_on_top INTEGER NOT NULL DEFAULT 0,
        is_default INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE folders (
        id INTEGER PRIMARY KEY,
        account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        path TEXT NOT NULL,
        name TEXT NOT NULL,
        parent_id INTEGER,
        role TEXT,
        delimiter TEXT,
        selectable INTEGER NOT NULL DEFAULT 1,
        depth INTEGER NOT NULL DEFAULT 0,
        uidvalidity INTEGER,
        uidnext INTEGER,
        highest_modseq INTEGER,
        last_sync_at INTEGER,
        oldest_synced_uid INTEGER,
        UNIQUE(account_id, path)
    );
    CREATE TABLE messages (
        id INTEGER PRIMARY KEY,
        account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
        uid INTEGER NOT NULL,
        message_id TEXT,
        thread_id TEXT NOT NULL,
        in_reply_to TEXT,
        refs TEXT NOT NULL DEFAULT '[]',
        subject TEXT NOT NULL DEFAULT '',
        base_subject TEXT NOT NULL DEFAULT '',
        from_name TEXT,
        from_email TEXT NOT NULL DEFAULT '',
        to_json TEXT NOT NULL DEFAULT '[]',
        cc_json TEXT NOT NULL DEFAULT '[]',
        bcc_json TEXT NOT NULL DEFAULT '[]',
        reply_to_json TEXT NOT NULL DEFAULT '[]',
        date INTEGER NOT NULL,
        snippet TEXT NOT NULL DEFAULT '',
        seen INTEGER NOT NULL DEFAULT 0,
        flagged INTEGER NOT NULL DEFAULT 0,
        answered INTEGER NOT NULL DEFAULT 0,
        forwarded INTEGER NOT NULL DEFAULT 0,
        draft INTEGER NOT NULL DEFAULT 0,
        keywords TEXT NOT NULL DEFAULT '[]',
        has_attachments INTEGER NOT NULL DEFAULT 0,
        size INTEGER NOT NULL DEFAULT 0,
        modseq INTEGER,
        gm_thrid INTEGER,
        gm_msgid INTEGER,
        list_unsub_json TEXT,
        auth_json TEXT,
        body_cached INTEGER NOT NULL DEFAULT 0,
        UNIQUE(folder_id, uid)
    );
    CREATE INDEX messages_folder_date ON messages(folder_id, date DESC);
    CREATE INDEX messages_thread ON messages(account_id, thread_id);
    CREATE INDEX messages_msgid ON messages(account_id, message_id);
    CREATE INDEX messages_from ON messages(from_email);
    CREATE INDEX messages_base_subject ON messages(account_id, base_subject);
    CREATE TABLE bodies (
        message_rowid INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
        raw BLOB NOT NULL
    );
    CREATE TABLE attachments (
        message_rowid INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
        part_id TEXT NOT NULL,
        filename TEXT NOT NULL,
        mime TEXT NOT NULL,
        size INTEGER NOT NULL,
        inline INTEGER NOT NULL,
        content_id TEXT,
        PRIMARY KEY(message_rowid, part_id)
    );
    CREATE TABLE thread_ids (
        account_id TEXT NOT NULL,
        message_id TEXT NOT NULL,
        thread_id TEXT NOT NULL,
        PRIMARY KEY(account_id, message_id)
    );
    CREATE INDEX thread_ids_thread ON thread_ids(account_id, thread_id);
    CREATE VIRTUAL TABLE messages_fts USING fts5(
        subject, sender, recipients, body,
        content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 2'
    );
    CREATE TABLE tags (
        key TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        color TEXT NOT NULL,
        sort_order INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE drafts (
        id TEXT PRIMARY KEY,
        json TEXT NOT NULL,
        updated_at INTEGER NOT NULL,
        server_folder_id INTEGER,
        server_uid INTEGER,
        server_message_id TEXT
    );
    CREATE TABLE outbox (
        id TEXT PRIMARY KEY,
        draft_json TEXT NOT NULL,
        state TEXT NOT NULL,
        send_at INTEGER NOT NULL,
        error TEXT,
        attempts INTEGER NOT NULL DEFAULT 0,
        next_attempt_at INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE contacts_seen (
        email TEXT PRIMARY KEY,
        name TEXT,
        count INTEGER NOT NULL DEFAULT 0,
        last_seen INTEGER NOT NULL DEFAULT 0,
        sent_to INTEGER NOT NULL DEFAULT 0
    );
    -- Source: Thunderbird mailnews.js pref("mailnews.tags.$label1.tag") … $label5 — the five default tags, so a
    -- Betterbird import keeps the same keyword names on the server.
    INSERT INTO tags(key, name, color, sort_order) VALUES
        ('$label1', 'Important', '#FF0000', 1),
        ('$label2', 'Work', '#FF9900', 2),
        ('$label3', 'Personal', '#009900', 3),
        ('$label4', 'To Do', '#3333FF', 4),
        ('$label5', 'Later', '#993399', 5);
    "#,
    // v2 — Gmail API / Microsoft Graph accounts (providers/). Source: docs/API.md "Engine notes → Providers".
    // remote_id: Gmail message id / Graph immutable message id (folders: Gmail label id / Graph folder id).
    // message_labels: a Gmail message lives in ONE row (in the account's "All Mail" folder) and belongs to every
    //   other label folder through this table — no duplicate rows or bodies per label.
    // sync_gen: generation stamp of the last full listing that saw the row; rows left behind after a completed
    //   full listing were deleted on the server while we were not watching (history expired / delta reset).
    r#"
    ALTER TABLE messages ADD COLUMN remote_id TEXT;
    ALTER TABLE messages ADD COLUMN sync_gen INTEGER NOT NULL DEFAULT 0;
    CREATE INDEX messages_remote ON messages(account_id, remote_id);
    ALTER TABLE folders ADD COLUMN remote_id TEXT;
    ALTER TABLE folders ADD COLUMN sync_state TEXT;
    CREATE INDEX folders_remote ON folders(account_id, remote_id);
    ALTER TABLE accounts ADD COLUMN api_state TEXT;
    ALTER TABLE drafts ADD COLUMN server_remote_id TEXT;
    CREATE TABLE message_labels (
        message_rowid INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
        folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
        PRIMARY KEY(message_rowid, folder_id)
    );
    CREATE INDEX message_labels_folder ON message_labels(folder_id);
    "#,
    // v3 — message filters and snooze (engine/rules.rs). Snoozes are local to this computer.
    r#"
    CREATE TABLE filters (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 1,
        account_id TEXT REFERENCES accounts(id) ON DELETE CASCADE,
        match_all INTEGER NOT NULL DEFAULT 1,
        conditions_json TEXT NOT NULL,
        actions_json TEXT NOT NULL,
        sort_order INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE snoozes (
        message_rowid INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
        until_ms INTEGER NOT NULL
    );
    CREATE INDEX snoozes_until ON snoozes(until_ms);
    -- Folder rows remember when they were created locally, so a folder-list reconcile that started before a folder
    -- was created (e.g. Drafts made by the first draft save while a sync pass was listing) cannot delete it.
    -- A trigger covers every insert path (IMAP list, folder_create, Gmail labels, Graph folders).
    ALTER TABLE folders ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;
    CREATE TRIGGER folders_created_at AFTER INSERT ON folders BEGIN
        UPDATE folders SET created_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) WHERE id = NEW.id;
    END;
    "#,
];

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    /// A second, read-only connection for what the user is looking at (the list, an open message, search). In WAL
    /// mode it reads the last committed state while a sync holds the writer, so a long background write can no longer
    /// leave a message on its loading skeleton or a search hanging (Paul, 28 Oct 2026: "its not opening the emails?
    /// and also search didnt work" — one engine thread held the only connection for ~45 s at a time). None for an
    /// in-memory store, where a second connection would be a different database.
    reader: Option<Arc<Mutex<Connection>>>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, MIGRATIONS)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, MIGRATIONS)
    }

    /// A store with its own migration list (calendar & contacts live in a separate file: pim/store.rs).
    pub fn open_with(path: &Path, migrations: &[&str]) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        let mut store = Self::init(conn, migrations)?;
        let reader = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        reader.execute_batch("PRAGMA query_only=ON; PRAGMA busy_timeout=5000;")?;
        store.reader = Some(Arc::new(Mutex::new(reader)));
        Ok(store)
    }

    pub fn open_in_memory_with(migrations: &[&str]) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, migrations)
    }

    fn init(conn: Connection, migrations: &[&str]) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in migrations.iter().enumerate() {
            let v = i as i64 + 1;
            if v > version {
                conn.execute_batch(&format!("BEGIN; {sql} PRAGMA user_version={v}; COMMIT;"))
                    .map_err(|e| ApiError::storage(format!("Could not upgrade the local database: {e}")))?;
            }
        }
        Ok(Store { conn: Arc::new(Mutex::new(conn)), reader: None })
    }

    /// Run `f` with the connection on the blocking pool.
    pub async fn call<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let mut guard = conn.lock();
            f(&mut guard)
        })
        .await?
    }

    /// Run a read-only `f` on the reader connection (the writer's when there is none), so it never waits behind a
    /// background write. `f` must not write: the connection is opened read-only and refuses.
    pub async fn read<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&Connection) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let conn = Arc::clone(self.reader.as_ref().unwrap_or(&self.conn));
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock();
            f(&guard)
        })
        .await?
    }

    /// Synchronous access for code already on a blocking thread (tests, import parsing).
    pub fn with<R>(&self, f: impl FnOnce(&mut Connection) -> Result<R>) -> Result<R> {
        let mut guard = self.conn.lock();
        f(&mut guard)
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_and_seeds_tags() {
        let s = Store::open_in_memory().unwrap();
        let n: i64 = s.with(|c| Ok(c.query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))?)).unwrap();
        assert_eq!(n, 5);
        let v: i64 = s.with(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?)).unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
    }

    #[tokio::test]
    async fn reader_sees_committed_rows_while_the_writer_is_held() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("t.sqlite3")).unwrap();
        s.with(|c| Ok(c.execute("INSERT INTO tags(key, name, color) VALUES ('x', 'X', '#000')", [])?)).unwrap();
        // Hold the writer (as a long sync batch would) and read through the reader meanwhile.
        let w = Arc::clone(&s.conn);
        let held = std::thread::spawn(move || {
            let g = w.lock();
            g.execute_batch("BEGIN IMMEDIATE; INSERT INTO tags(key, name, color) VALUES ('y', 'Y', '#111');").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(400));
            g.execute_batch("COMMIT;").unwrap();
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        let t0 = std::time::Instant::now();
        let n: i64 = s.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM tags WHERE key IN ('x','y')", [], |r| r.get(0))?)).await.unwrap();
        assert!(t0.elapsed() < std::time::Duration::from_millis(300), "the reader waited for the writer");
        assert_eq!(n, 1, "only committed rows are seen");
        held.join().unwrap();
        // And it can never write.
        assert!(s.read(|c| Ok(c.execute("DELETE FROM tags", [])?)).await.is_err());
    }

    #[test]
    fn fts_contentless_delete_works() {
        let s = Store::open_in_memory().unwrap();
        s.with(|c| {
            c.execute("INSERT INTO messages_fts(rowid, subject, sender, recipients, body) VALUES (7,'Invoice March','a@b.c','x@y.z','pay now')", [])?;
            let hit: i64 = c.query_row("SELECT rowid FROM messages_fts WHERE messages_fts MATCH 'invoice'", [], |r| r.get(0))?;
            assert_eq!(hit, 7);
            c.execute("DELETE FROM messages_fts WHERE rowid=7", [])?;
            let n: i64 = c.query_row("SELECT COUNT(*) FROM messages_fts WHERE messages_fts MATCH 'invoice'", [], |r| r.get(0))?;
            assert_eq!(n, 0);
            Ok(())
        })
        .unwrap();
    }
}
