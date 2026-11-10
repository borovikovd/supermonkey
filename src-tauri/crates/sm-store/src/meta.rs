//! The `meta` row: the last committed `rev` and the last action number. Both
//! survive restarts and never go back (SPEC UI state, Actions).

use rusqlite::Connection;
use sm_domain::{ActionNo, Rev};

use crate::StoreError;
use crate::db::Db;
use crate::tx::WriteTx;

/// Reads of the `meta` row.
#[derive(Debug, Clone, Copy)]
pub struct MetaReads<'c> {
    db: Db<'c>,
}

impl<'c> MetaReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// The `rev` of the last commit that wrote.
    pub fn rev(self) -> Result<Rev, StoreError> {
        self.db.one(
            "read the rev",
            "SELECT rev FROM meta WHERE id = 1",
            [],
            |r| r.get("rev"),
        )
    }

    /// The highest action number applied.
    pub fn last_action(self) -> Result<ActionNo, StoreError> {
        self.db.one(
            "read the last action number",
            "SELECT last_action FROM meta WHERE id = 1",
            [],
            |r| r.get("last_action"),
        )
    }
}

/// Writes to the `meta` row.
#[derive(Debug)]
pub struct MetaWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> MetaWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Records that actions up to `n` are applied. It never goes back: a
    /// lower number is ignored. Not a row change, so on its own it does not
    /// move `rev`.
    pub fn set_last_action(&self, n: ActionNo) -> Result<(), StoreError> {
        self.tx.db().execute(
            "store the last action number",
            "UPDATE meta SET last_action = max(last_action, ?1) WHERE id = 1",
            [n],
        )?;
        Ok(())
    }
}

/// The stored `rev`, read when the writer starts.
pub(crate) fn stored_rev(conn: &Connection) -> Result<Rev, StoreError> {
    MetaReads::new(Db::new(conn)).rev()
}

/// Stores the `rev` of the transaction about to commit.
pub(crate) fn store_rev(conn: &Connection, rev: Rev) -> Result<(), StoreError> {
    Db::new(conn).execute(
        "store the rev",
        "UPDATE meta SET rev = ?1 WHERE id = 1",
        [rev],
    )?;
    Ok(())
}

/// Leaves a deferred foreign key broken, so the coming `COMMIT` fails as a
/// real constraint or I/O failure would.
#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn break_commit(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "PRAGMA defer_foreign_keys = ON;
         INSERT INTO thread_labels (thread_id, account_id, label_id) VALUES (-1, 0, 'broken');",
    )
    .map_err(StoreError::sql("break the commit on purpose"))
}
