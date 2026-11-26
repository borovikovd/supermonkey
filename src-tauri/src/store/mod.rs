mod accounts;
mod actor;
mod contacts;
mod mail;
mod queues;
mod search;
mod sql;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, ErrorCode, OpenFlags, Transaction};
use thiserror::Error;

use crate::domain::{AccountId, DraftId, Identity, LabelId, Rev, Router, ThreadId};

pub use accounts::{AccountRecord, LabelRecord, NewNotice, NoticeRecord};
pub use actor::{Pending, Priority, ReadPool, Writer};
pub use contacts::ContactSighting;
pub use mail::{
    AttachmentRecord, ConversationView, MessageBodyRecord, MessageBodyWrite, MessageRecord,
    NewAttachment, NewMessage, ThreadRow, ThreadSyncState,
};
pub use queues::{
    DraftAttachmentItem, DraftRecord, DraftWrite, NewDraft, NewOp, NewOutbox, OpRecord, OutboxItem,
    OutboxRecord, UndoAction, UndoRecord,
};
pub use sql::Db;

pub const SCHEMA_VERSION: i32 = 8;
pub const SEARCH_LIMIT: usize = 500;
pub const CANDIDATE_LIMIT: usize = 5_000;
pub const UNDO_KEPT: usize = 50;

const SCHEMA_SQL: &str = include_str!("schema.sql");
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const STMT_CACHE_CAP: usize = 256;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("SQLite error while {what}: {source}")]
    Sqlite {
        what: &'static str,
        #[source]
        source: rusqlite::Error,
    },
    #[error("couldn't {action} {}: {source}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("JSON codec error in store: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("{0}")]
    Invalid(&'static str),
    #[error("the database writer has stopped")]
    WriterStopped,
    #[error("writer job panicked: {0}")]
    JobPanicked(String),
}

impl StoreError {
    pub(crate) const fn sql(what: &'static str) -> impl FnOnce(rusqlite::Error) -> Self {
        move |source| Self::Sqlite { what, source }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Table {
    Accounts,
    Labels,
    Splits,
    Pins,
    Threads,
    Messages,
    Attachments,
    Drafts,
    Outbox,
    Ops,
    Undo,
    Notices,
    Settings,
    SyncCursor,
}

impl Table {
    pub const ALL: [Self; 14] = [
        Self::Accounts,
        Self::Labels,
        Self::Splits,
        Self::Pins,
        Self::Threads,
        Self::Messages,
        Self::Attachments,
        Self::Drafts,
        Self::Outbox,
        Self::Ops,
        Self::Undo,
        Self::Notices,
        Self::Settings,
        Self::SyncCursor,
    ];
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Changes {
    pub threads: HashSet<ThreadId>,
    pub drafts: HashSet<DraftId>,
    pub tables: HashMap<Option<AccountId>, HashSet<Table>>,
}

impl Changes {
    pub fn touch(&mut self, account: Option<AccountId>, table: Table) {
        self.tables.entry(account).or_default().insert(table);
    }

    pub fn thread(&mut self, account: AccountId, thread: ThreadId) {
        self.threads.insert(thread);
        self.touch(Some(account), Table::Threads);
    }

    pub fn draft(&mut self, account: AccountId, draft: DraftId) {
        self.drafts.insert(draft);
        self.touch(Some(account), Table::Drafts);
    }

    #[must_use]
    pub fn has_table(&self, account: Option<AccountId>, table: Table) -> bool {
        self.tables
            .get(&account)
            .is_some_and(|set| set.contains(&table))
    }

    #[must_use]
    pub fn has_any_account_table(&self, table: Table) -> bool {
        self.tables.values().any(|set| set.contains(&table))
    }

    #[must_use]
    pub fn touches_any(&self, account: Option<AccountId>, tables: &[Table]) -> bool {
        let Some(set) = self.tables.get(&account) else {
            return false;
        };
        tables.iter().any(|t| set.contains(t))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub rev: Rev,
    pub changes: Changes,
}

pub(super) struct CachedRouting {
    pub(super) router: Router,
    pub(super) identity: Identity,
    pub(super) user_labels: HashMap<LabelId, String>,
}

pub struct WriteTx<'c> {
    tx: &'c Transaction<'c>,
    next_rev: Rev,
    wrote: bool,
    changes: Changes,
    routing: HashMap<AccountId, CachedRouting>,
    #[cfg(any(test, feature = "fake"))]
    fail_commit: bool,
}

impl std::fmt::Debug for WriteTx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteTx")
            .field("next_rev", &self.next_rev)
            .field("wrote", &self.wrote)
            .finish_non_exhaustive()
    }
}

pub(super) struct Finished {
    pub(super) wrote: bool,
    pub(super) changes: Changes,
    #[cfg(any(test, feature = "fake"))]
    pub(super) fail_commit: bool,
}

impl<'c> WriteTx<'c> {
    pub(super) fn new(tx: &'c Transaction<'c>, next_rev: Rev) -> Self {
        Self {
            tx,
            next_rev,
            wrote: false,
            changes: Changes::default(),
            routing: HashMap::new(),
            #[cfg(any(test, feature = "fake"))]
            fail_commit: false,
        }
    }

    #[must_use]
    pub fn read(&self) -> Db<'_> {
        Db(self.tx)
    }

    pub const fn stamp(&mut self) -> Rev {
        self.wrote = true;
        self.next_rev
    }

    pub const fn record(&mut self) -> &mut Changes {
        self.wrote = true;
        &mut self.changes
    }

    pub fn touch(&mut self, account: Option<AccountId>, table: Table) {
        self.wrote = true;
        self.changes.touch(account, table);
    }

    pub(super) fn invalidate_routing(&mut self, account: AccountId) {
        self.routing.remove(&account);
    }

    #[cfg(any(test, feature = "fake"))]
    pub const fn fail_commit(&mut self) {
        self.fail_commit = true;
    }

    pub fn savepoint<T, E>(&mut self, run: impl FnOnce(&mut Self) -> Result<T, E>) -> Result<T, E>
    where
        E: From<StoreError>,
    {
        self.tx
            .execute_batch("SAVEPOINT sm_step")
            .map_err(StoreError::sql("begin savepoint"))?;
        let saved_wrote = self.wrote;
        let saved_changes = self.changes.clone();
        match run(self) {
            Ok(value) => {
                self.tx
                    .execute_batch("RELEASE SAVEPOINT sm_step")
                    .map_err(StoreError::sql("release savepoint"))?;
                Ok(value)
            }
            Err(err) => {
                self.wrote = saved_wrote;
                self.changes = saved_changes;
                self.routing.clear();
                if let Err(rb_err) = self
                    .tx
                    .execute_batch("ROLLBACK TO SAVEPOINT sm_step; RELEASE SAVEPOINT sm_step")
                {
                    tracing::warn!(error = %rb_err, "failed to roll back savepoint");
                }
                Err(err)
            }
        }
    }

    pub(super) fn finish(self) -> Finished {
        Finished {
            wrote: self.wrote,
            changes: self.changes,
            #[cfg(any(test, feature = "fake"))]
            fail_commit: self.fail_commit,
        }
    }
}

pub fn with_write_tx<T>(
    path: &Path,
    run: impl FnOnce(&mut WriteTx<'_>) -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    let mut conn = open_writer(path)?;
    let next_rev = Rev(Db(&conn).rev()?.0 + 1);
    let tx = conn
        .transaction()
        .map_err(StoreError::sql("begin transaction"))?;
    let mut wtx = WriteTx::new(&tx, next_rev);
    let out = run(&mut wtx)?;
    let finished = wtx.finish();
    if finished.wrote {
        Db(&tx).execute(
            "UPDATE meta SET rev = ?1 WHERE id = 1",
            [next_rev],
            "update meta rev",
        )?;
    }
    tx.commit().map_err(StoreError::sql("commit transaction"))?;
    Ok(out)
}

pub fn open_writer(path: &Path) -> Result<Connection, StoreError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|source| StoreError::Io {
            action: "create folder for",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    if path.exists() {
        match try_open_existing(path) {
            Ok(Some(conn)) => return Ok(conn),
            Ok(None) => move_aside(path)?,
            Err(err) => return Err(err),
        }
    }
    create_fresh(path)
}

pub fn open_reader(path: &Path) -> Result<Connection, StoreError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags)
        .map_err(StoreError::sql("open a read database connection"))?;
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(StoreError::sql("set reader busy timeout"))?;
    conn.set_prepared_statement_cache_capacity(STMT_CACHE_CAP);
    conn.execute_batch(
        "PRAGMA temp_store = MEMORY; PRAGMA cache_size = -16000; PRAGMA mmap_size = 268435456;",
    )
    .map_err(StoreError::sql("configure reader pragmas"))?;
    Ok(conn)
}

fn try_open_existing(path: &Path) -> Result<Option<Connection>, StoreError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = match Connection::open_with_flags(path, flags) {
        Ok(c) => c,
        Err(e) if is_corrupt(&e) => return Ok(None),
        Err(e) => return Err(StoreError::sql("open the existing database")(e)),
    };
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(StoreError::sql("set busy timeout"))?;
    let version: i32 = match conn.pragma_query_value(None, "user_version", |r| r.get(0)) {
        Ok(v) => v,
        Err(e) if is_corrupt(&e) => return Ok(None),
        Err(e) => return Err(StoreError::sql("read user_version")(e)),
    };
    if version != SCHEMA_VERSION {
        return Ok(None);
    }
    configure_writer(&conn)?;
    Ok(Some(conn))
}

fn create_fresh(path: &Path) -> Result<Connection, StoreError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags)
        .map_err(StoreError::sql("create a fresh database"))?;
    configure_writer(&conn)?;
    conn.execute_batch(SCHEMA_SQL)
        .map_err(StoreError::sql("initialize database schema"))?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(StoreError::sql("set schema user_version"))?;
    Ok(conn)
}

fn configure_writer(conn: &Connection) -> Result<(), StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT)
        .map_err(StoreError::sql("set writer busy timeout"))?;
    conn.set_prepared_statement_cache_capacity(STMT_CACHE_CAP);
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA temp_store = MEMORY;
         PRAGMA cache_size = -32000;
         PRAGMA mmap_size = 268435456;",
    )
    .map_err(StoreError::sql("configure writer pragmas"))?;
    Ok(())
}

fn is_corrupt(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt)
    )
}

fn move_aside(path: &Path) -> Result<(), StoreError> {
    let mut n = 1u32;
    let target = loop {
        let mut candidate = path.as_os_str().to_owned();
        candidate.push(format!(".old-{n}"));
        let p = PathBuf::from(candidate);
        if !p.exists() {
            break p;
        }
        n = n.saturating_add(1);
    };
    fs::rename(path, &target).map_err(|source| StoreError::Io {
        action: "move aside old database",
        path: path.to_path_buf(),
        source,
    })?;
    for suffix in ["-wal", "-shm"] {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        let side_path = PathBuf::from(side);
        if side_path.exists() {
            let mut dest = target.as_os_str().to_owned();
            dest.push(suffix);
            if let Err(err) = fs::rename(&side_path, PathBuf::from(dest)) {
                tracing::warn!(error = %err, "failed to move aside SQLite sidecar");
            }
        }
    }
    Ok(())
}
