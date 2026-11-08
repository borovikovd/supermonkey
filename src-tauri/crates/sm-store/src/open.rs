//! Opening the database: the schema, its version guard, and connection tuning.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use crate::StoreError;

/// The `user_version` of a database made by this `schema.sql`. Change it with
/// any change to the schema, or to what stored rows mean, so they are read
/// from Gmail again: a file with another version is moved aside and a fresh
/// one created, since there are no migrations (ARCHITECTURE.md, section 6).
/// 5: only images are inline parts, so files sent `inline` are listed.
/// 6: an image is inline only when the HTML body shows it, so a photo sent
/// `inline` with no Content-ID is listed, and stored inside an attached
/// message.
pub const SCHEMA_VERSION: i32 = 7;

const SCHEMA: &str = include_str!("schema.sql");

/// Statements kept prepared per connection. The repositories have about a
/// hundred; a smaller cache would re-prepare the hot ones.
const STATEMENT_CACHE: usize = 256;

/// How long a connection waits for a lock before failing. Only a checkpoint
/// ever holds one against the single writer, so this is a backstop.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Opens the write connection, creating the database when there is none and
/// moving aside one with another schema version, or a file that is not a
/// database. Any other error reading the file fails, and leaves it in place.
pub(crate) fn open_writer(path: &Path) -> Result<Connection, StoreError> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|source| StoreError::Io {
            action: "create the folder",
            path: dir.to_path_buf(),
            source,
        })?;
    }
    if path.exists() {
        match stored_version(path) {
            Ok(SCHEMA_VERSION) => return open_existing(path),
            Ok(found) => {
                let aside = move_aside(path)?;
                tracing::warn!(
                    target: "sm_store",
                    found, expected = SCHEMA_VERSION, moved_to = %aside.display(),
                    "database has another schema version; starting a fresh one",
                );
            }
            Err(e) if is_not_a_database(&e) => {
                let aside = move_aside(path)?;
                tracing::warn!(
                    target: "sm_store",
                    error = %e, moved_to = %aside.display(),
                    "database file is not a database or is corrupt; starting a fresh one",
                );
            }
            // Locked, unreadable, or out of space: the mail is still there,
            // so startup fails rather than throw it away.
            Err(e) => return Err(StoreError::sql("read the database's schema version")(e)),
        }
    }
    create(path)
}

/// Opens a read-only connection for the read pool. The writer must have
/// created the database first.
pub(crate) fn open_reader(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(StoreError::sql("open a read connection"))?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .and_then(|()| {
            conn.execute_batch(
                "PRAGMA temp_store = MEMORY;
                 PRAGMA cache_size = -16000;
                 PRAGMA mmap_size = 268435456;",
            )
        })
        .map_err(StoreError::sql("tune a read connection"))?;
    conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
    Ok(conn)
}

fn stored_version(path: &Path) -> rusqlite::Result<i32> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// Whether SQLite found the file is no database it can use, the one error
/// that moves it aside.
fn is_not_a_database(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt)
    )
}

fn open_existing(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open(path).map_err(StoreError::sql("open the database"))?;
    tune_writer(&conn)?;
    Ok(conn)
}

fn create(path: &Path) -> Result<Connection, StoreError> {
    let mut conn = Connection::open(path).map_err(StoreError::sql("create the database"))?;
    tune_writer(&conn)?;
    // One transaction, version included: a crash part way leaves version 0,
    // and the next start moves that file aside too.
    let tx = conn
        .transaction()
        .map_err(StoreError::sql("begin creating the schema"))?;
    tx.execute_batch(SCHEMA)
        .and_then(|()| tx.pragma_update(None, "user_version", SCHEMA_VERSION))
        .and_then(|()| tx.commit())
        .map_err(StoreError::sql("create the schema"))?;
    Ok(conn)
}

fn tune_writer(conn: &Connection) -> Result<(), StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT)
        .and_then(|()| {
            conn.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;
                 PRAGMA foreign_keys = ON;
                 PRAGMA temp_store = MEMORY;
                 PRAGMA cache_size = -32000;
                 PRAGMA mmap_size = 268435456;",
            )
        })
        .map_err(StoreError::sql("tune the write connection"))?;
    conn.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
    Ok(())
}

/// Renames the file, and its WAL and shared-memory files, to the first free
/// `<name>.old-<n>`. Returns the new path.
fn move_aside(path: &Path) -> Result<PathBuf, StoreError> {
    let mut n = 1_u32;
    let aside = loop {
        let candidate = sibling(path, &format!(".old-{n}"));
        if !candidate.exists() {
            break candidate;
        }
        n = n.saturating_add(1);
    };
    rename(path, &aside)?;
    for suffix in ["-wal", "-shm"] {
        let from = sibling(path, suffix);
        if from.exists() {
            rename(&from, &sibling(&aside, suffix))?;
        }
    }
    Ok(aside)
}

/// `path` with `suffix` appended to its file name.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn rename(from: &Path, to: &Path) -> Result<(), StoreError> {
    std::fs::rename(from, to).map_err(|source| StoreError::Io {
        action: "move aside",
        path: from.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(path: &Path) -> i32 {
        stored_version(path).unwrap()
    }

    #[test]
    fn a_fresh_database_gets_the_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("mail.db");
        drop(open_writer(&path).unwrap());
        assert_eq!(version(&path), SCHEMA_VERSION);
    }

    #[test]
    fn a_current_database_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.db");
        {
            let conn = open_writer(&path).unwrap();
            conn.execute("UPDATE meta SET rev = 41", []).unwrap();
        }
        let conn = open_writer(&path).unwrap();
        let rev: i64 = conn
            .query_row("SELECT rev FROM meta", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rev, 41);
        assert!(!sibling(&path, ".old-1").exists());
    }

    #[test]
    fn another_version_is_moved_aside_with_its_wal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;
                 CREATE TABLE old (x); INSERT INTO old VALUES (7); PRAGMA user_version = 99;",
            )
            .unwrap();
            // Keep the WAL file: closing the last connection would checkpoint it.
            std::mem::forget(conn);
        }
        assert!(sibling(&path, "-wal").exists());
        // An earlier move-aside is kept, not overwritten.
        std::fs::write(sibling(&path, ".old-1"), b"older").unwrap();

        drop(open_writer(&path).unwrap());

        assert_eq!(version(&path), SCHEMA_VERSION);
        assert_eq!(std::fs::read(sibling(&path, ".old-1")).unwrap(), b"older");
        let aside = sibling(&path, ".old-2");
        assert!(
            sibling(&aside, "-wal").exists(),
            "the WAL moves with its file"
        );
        assert_eq!(version(&aside), 99);
        let kept: i64 = Connection::open(&aside)
            .unwrap()
            .query_row("SELECT x FROM old", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, 7, "the old file keeps its rows");
    }

    #[test]
    fn a_file_that_is_not_a_database_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.db");
        std::fs::write(
            &path,
            b"not sqlite at all, but long enough to have a header....",
        )
        .unwrap();
        drop(open_writer(&path).unwrap());
        assert_eq!(version(&path), SCHEMA_VERSION);
        assert!(sibling(&path, ".old-1").exists());
    }

    #[test]
    fn a_locked_database_is_left_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.db");
        {
            let conn = open_writer(&path).unwrap();
            conn.execute("UPDATE meta SET rev = 41", []).unwrap();
        }
        // Another connection holds the file's lock throughout.
        let holder = Connection::open(&path).unwrap();
        holder
            .execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;")
            .unwrap();
        assert!(open_writer(&path).is_err(), "startup fails loudly");
        assert!(!sibling(&path, ".old-1").exists(), "not moved aside");
        drop(holder);

        let conn = open_writer(&path).unwrap();
        let rev: i64 = conn
            .query_row("SELECT rev FROM meta", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rev, 41, "the same database, not a fresh one");
    }

    #[test]
    fn a_database_that_cannot_be_opened_is_left_in_place() {
        let dir = tempfile::tempdir().unwrap();
        // Something SQLite can't open, though it is no corrupt database.
        let path = dir.path().join("mail.db");
        std::fs::create_dir(&path).unwrap();

        assert!(open_writer(&path).is_err(), "startup fails loudly");
        assert!(path.is_dir(), "left where it was");
        assert!(!sibling(&path, ".old-1").exists());
    }
}
