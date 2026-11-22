use rusqlite::Connection;
use sm_domain::{ActionNo, Rev};

use crate::StoreError;
use crate::db::Db;
use crate::tx::WriteTx;

#[derive(Debug, Clone, Copy)]
pub struct MetaReads<'c> {
    db: Db<'c>,
}

impl<'c> MetaReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    pub fn rev(self) -> Result<Rev, StoreError> {
        self.db.one(
            "read the rev",
            "SELECT rev FROM meta WHERE id = 1",
            [],
            |r| r.get("rev"),
        )
    }

    pub fn last_action(self) -> Result<ActionNo, StoreError> {
        self.db.one(
            "read the last action number",
            "SELECT last_action FROM meta WHERE id = 1",
            [],
            |r| r.get("last_action"),
        )
    }
}

#[derive(Debug)]
pub struct MetaWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> MetaWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    pub fn set_last_action(&self, n: ActionNo) -> Result<(), StoreError> {
        self.tx.db().execute(
            "store the last action number",
            "UPDATE meta SET last_action = max(last_action, ?1) WHERE id = 1",
            [n],
        )?;
        Ok(())
    }
}

pub(crate) fn stored_rev(conn: &Connection) -> Result<Rev, StoreError> {
    MetaReads::new(Db::new(conn)).rev()
}

pub(crate) fn store_rev(conn: &Connection, rev: Rev) -> Result<(), StoreError> {
    Db::new(conn).execute(
        "store the rev",
        "UPDATE meta SET rev = ?1 WHERE id = 1",
        [rev],
    )?;
    Ok(())
}

#[cfg(any(test, feature = "test-hooks"))]
pub(crate) fn break_commit(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "PRAGMA defer_foreign_keys = ON;
         INSERT INTO thread_labels (thread_id, account_id, label_id) VALUES (-1, 0, 'broken');",
    )
    .map_err(StoreError::sql("break the commit on purpose"))
}
