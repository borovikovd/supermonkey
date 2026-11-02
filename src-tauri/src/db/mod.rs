//! SQLite access. One writer connection lives on the writer thread; reads use a
//! small pool of separate connections in WAL mode.

pub mod queries;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{Connection, OpenFlags};

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
    Ok(conn)
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
#[derive(Clone)]
pub struct ReadPool {
    path: PathBuf,
    conns: Arc<Vec<Mutex<Option<Connection>>>>,
}

impl ReadPool {
    pub fn new(path: &Path, size: usize) -> ReadPool {
        ReadPool {
            path: path.to_path_buf(),
            conns: Arc::new((0..size).map(|_| Mutex::new(None)).collect()),
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

/// Added to the wall clock. Only the E2E server moves it, to make app time pass.
static CLOCK_OFFSET_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// App time in Unix milliseconds. Every time check in the core reads this.
pub fn now_ms() -> i64 {
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    wall.saturating_add(CLOCK_OFFSET_MS.load(std::sync::atomic::Ordering::Relaxed))
}

/// App time in the local time zone.
pub fn now_local() -> chrono::DateTime<chrono::Local> {
    chrono::DateTime::from_timestamp_millis(now_ms())
        .map_or_else(chrono::Local::now, |d| d.with_timezone(&chrono::Local))
}

/// Moves app time forward (E2E tests).
pub fn advance_clock(ms: i64) {
    CLOCK_OFFSET_MS.fetch_add(ms, std::sync::atomic::Ordering::Relaxed);
}

/// JSON column helpers.
pub fn to_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

pub fn from_json<T: serde::de::DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_str(s).unwrap_or_default()
}
