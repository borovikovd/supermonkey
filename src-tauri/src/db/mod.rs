//! SQLite access. One writer connection lives on the writer thread; reads use a
//! small pool of separate connections in WAL mode.

pub mod queries;

use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use rusqlite::{Connection, OpenFlags, OptionalExtension as _};

const SCHEMA: &str = include_str!("schema.sql");

fn tune(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA temp_store = MEMORY;
         PRAGMA cache_size = -32000;
         PRAGMA mmap_size = 268435456;",
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

/// Opens the writer connection and creates the schema.
pub fn open_writer(path: &Path) -> anyhow::Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    tune(&conn)?;
    conn.execute_batch(SCHEMA)?;
    for table in ["notices", "outbox"] {
        add_autoincrement(&conn, table)?;
    }
    Ok(conn)
}

/// The UI keys notices and send toasts by id, so those ids are never reused.
/// Databases from before these tables had AUTOINCREMENT get them rebuilt with
/// their rows. Neither table has indexes or tables referencing it.
fn add_autoincrement(conn: &Connection, table: &str) -> rusqlite::Result<()> {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |r| r.get(0),
        )
        .optional()?;
    if sql.is_none_or(|s| s.contains("AUTOINCREMENT")) {
        return Ok(());
    }
    // One transaction, so a crash can't strand the rows in `{table}_old`.
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(&format!("ALTER TABLE {table} RENAME TO {table}_old"))?;
    tx.execute_batch(SCHEMA)?;
    tx.execute_batch(&format!(
        "INSERT INTO {table} SELECT * FROM {table}_old; DROP TABLE {table}_old"
    ))?;
    tx.commit()
}

fn open_reader(path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    tune(&conn)?;
    Ok(conn)
}

/// A few read-only connections, handed out round-robin.
pub struct ReadPool {
    path: PathBuf,
    conns: Box<[Mutex<Option<Connection>>]>,
}

impl ReadPool {
    pub fn new(path: &Path, size: usize) -> ReadPool {
        ReadPool {
            path: path.to_path_buf(),
            conns: (0..size).map(|_| Mutex::new(None)).collect(),
        }
    }

    /// Runs `f` on a free connection, waiting for one if all are busy.
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let mut guard = self
            .conns
            .iter()
            .find_map(|slot| slot.try_lock())
            .unwrap_or_else(|| self.conns[0].lock());
        let conn = match &mut *guard {
            Some(conn) => conn,
            slot => slot.insert(open_reader(&self.path)?),
        };
        f(conn)
    }
}

/// JSON column helpers.
pub fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

pub fn from_json<T: serde::de::DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_str(s).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database from before AUTOINCREMENT keeps its rows and stops reusing ids.
    #[test]
    fn old_outbox_and_notices_get_autoincrement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        {
            let conn = open_writer(&path).unwrap();
            conn.execute_batch(
                "DROP TABLE outbox; DROP TABLE notices;
                 CREATE TABLE outbox (id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL,
                   draft_id TEXT NOT NULL, thread_id INTEGER, send_after INTEGER NOT NULL,
                   state TEXT NOT NULL, gmail_draft_id TEXT, sent_message_id TEXT,
                   undo_requested INTEGER NOT NULL DEFAULT 0, toast_closed INTEGER NOT NULL DEFAULT 0,
                   delayed INTEGER NOT NULL DEFAULT 0, error TEXT, rev INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE notices (id INTEGER PRIMARY KEY, account_id INTEGER NOT NULL,
                   kind TEXT NOT NULL, message TEXT NOT NULL, draft_id TEXT, thread_id INTEGER,
                   created_at INTEGER NOT NULL);
                 INSERT INTO accounts (id, email, position, hue) VALUES (1, 'a@x.test', 0, 0);
                 INSERT INTO outbox (id, account_id, draft_id, send_after, state) VALUES (7, 1, 'd', 0, 'waiting');
                 INSERT INTO notices (id, account_id, kind, message, created_at) VALUES (3, 1, 'info', 'hi', 0);",
            )
            .unwrap();
        }
        let conn = open_writer(&path).unwrap();
        let kept: String = conn
            .query_row("SELECT draft_id FROM outbox WHERE id = 7", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, "d");
        conn.execute_batch(
            "DELETE FROM outbox; DELETE FROM notices;
             INSERT INTO outbox (account_id, draft_id, send_after, state) VALUES (1, 'e', 0, 'waiting');
             INSERT INTO notices (account_id, kind, message, created_at) VALUES (1, 'info', 'again', 0);",
        )
        .unwrap();
        let outbox: i64 = conn
            .query_row("SELECT id FROM outbox", [], |r| r.get(0))
            .unwrap();
        let notice: i64 = conn
            .query_row("SELECT id FROM notices", [], |r| r.get(0))
            .unwrap();
        assert!(
            outbox > 7 && notice > 3,
            "ids reused: outbox {outbox}, notice {notice}"
        );
        let leftovers: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name LIKE '%_old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leftovers, 0);

        // Opening a migrated database again must not rebuild the tables,
        // which would reset their sequences and reuse ids once rows are gone.
        conn.execute_batch("DELETE FROM outbox; DELETE FROM notices;")
            .unwrap();
        drop(conn);
        let conn = open_writer(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO outbox (account_id, draft_id, send_after, state) VALUES (1, 'f', 0, 'waiting');
             INSERT INTO notices (account_id, kind, message, created_at) VALUES (1, 'info', 'later', 0);",
        )
        .unwrap();
        let outbox_again: i64 = conn
            .query_row("SELECT id FROM outbox", [], |r| r.get(0))
            .unwrap();
        let notice_again: i64 = conn
            .query_row("SELECT id FROM notices", [], |r| r.get(0))
            .unwrap();
        assert!(
            outbox_again > outbox && notice_again > notice,
            "ids reused after reopening: outbox {outbox_again}, notice {notice_again}"
        );
    }
}
