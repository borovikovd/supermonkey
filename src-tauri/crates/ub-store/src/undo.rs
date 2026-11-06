//! The undo history (SPEC Undo): the last 50 entries per account, each keyed
//! by the number of the action that made it.

use rusqlite::{Row, named_params};
use serde::{Deserialize, Serialize};
use ub_domain::{
    AccountId, OutboxId, PinKind, SplitId, ThreadChange, ThreadId, UndoEntryId, UndoKind,
};

use crate::db::{Db, json_column, to_json};
use crate::tx::WriteTx;
use crate::{StoreError, Table};

/// How many entries each account keeps.
pub const UNDO_KEPT: u32 = 50;

/// What undoing an entry reverts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UndoAction {
    /// A mail action: the labels and snooze times it changed, with their
    /// values from before it.
    Mail {
        /// One entry per thread it touched.
        threads: Vec<ThreadChange>,
    },
    /// A send: its outbox row.
    Send {
        /// The row.
        outbox: OutboxId,
    },
    /// A pin: the one it set and the split it replaced.
    Pin {
        /// What the pin matches.
        pin_kind: PinKind,
        /// The lowercased sender, domain, or label name.
        value: String,
        /// The split it pinned to.
        split: SplitId,
        /// The split the same pin had before, if any.
        replaced: Option<SplitId>,
    },
}

impl UndoAction {
    /// The entry's kind.
    pub fn kind(&self) -> UndoKind {
        match self {
            Self::Mail { .. } => UndoKind::Mail,
            Self::Send { .. } => UndoKind::Send,
            Self::Pin { .. } => UndoKind::Pin,
        }
    }

    /// The threads a mail entry changed.
    pub fn threads(&self) -> Vec<ThreadId> {
        match self {
            Self::Mail { threads } => threads.iter().map(|t| t.thread).collect(),
            Self::Send { .. } | Self::Pin { .. } => Vec::new(),
        }
    }

    /// A send entry's outbox row.
    pub fn outbox(&self) -> Option<OutboxId> {
        match self {
            Self::Send { outbox } => Some(*outbox),
            Self::Mail { .. } | Self::Pin { .. } => None,
        }
    }
}

/// One stored entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoRecord {
    /// The number of the action that made it.
    pub id: UndoEntryId,
    /// Its account.
    pub account: AccountId,
    /// What the toast and history call it.
    pub label: String,
    /// What undoing it reverts.
    pub action: UndoAction,
}

fn undo_row(r: &Row<'_>) -> rusqlite::Result<UndoRecord> {
    Ok(UndoRecord {
        id: r.get("id")?,
        account: r.get("account_id")?,
        label: r.get("label")?,
        action: json_column(r, "payload")?,
    })
}

/// Reads of the undo history.
#[derive(Clone, Copy)]
pub struct UndoReads<'c> {
    db: Db<'c>,
}

impl<'c> UndoReads<'c> {
    pub(crate) fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// The account's entries, oldest first.
    pub fn entries(self, account: AccountId) -> Result<Vec<UndoRecord>, StoreError> {
        self.db.all(
            "read the undo history",
            "SELECT id, account_id, label, payload FROM undo_entries
             WHERE account_id = ?1 ORDER BY id",
            [account],
            undo_row,
        )
    }

    /// One entry of the account; `None` when it is gone, so an undo of it is
    /// refused.
    pub fn get(
        self,
        account: AccountId,
        id: UndoEntryId,
    ) -> Result<Option<UndoRecord>, StoreError> {
        self.db.optional(
            "read an undo entry",
            "SELECT id, account_id, label, payload FROM undo_entries
             WHERE id = ?1 AND account_id = ?2",
            (id, account),
            undo_row,
        )
    }

    /// The entries of sends whose row is already sent: dropped on launch, so
    /// a stray `z` never trashes delivered mail.
    pub fn for_sent_rows(self) -> Result<Vec<UndoEntryId>, StoreError> {
        self.db.all(
            "read the sent sends' undo entries",
            "SELECT u.id FROM undo_entries u JOIN outbox o ON o.id = u.outbox_id
             WHERE o.state = 'sent' ORDER BY u.id",
            [],
            |r| r.get("id"),
        )
    }
}

/// Writes to the undo history.
pub struct UndoWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> UndoWrites<'t, 'c> {
    pub(crate) fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Adds an entry, replacing one with the same id, and drops the account's
    /// entries beyond the newest [`UNDO_KEPT`].
    pub fn record(
        &mut self,
        account: AccountId,
        id: UndoEntryId,
        label: &str,
        action: &UndoAction,
    ) -> Result<(), StoreError> {
        let db = self.tx.db();
        db.execute(
            "add an undo entry",
            "INSERT OR REPLACE INTO undo_entries (id, account_id, kind, label, outbox_id, payload)
             VALUES (:id, :account, :kind, :label, :outbox, :payload)",
            named_params! {
                ":id": id, ":account": account, ":kind": action.kind(), ":label": label,
                ":outbox": action.outbox(), ":payload": to_json("payload", action)?,
            },
        )?;
        db.execute(
            "trim the undo history",
            "DELETE FROM undo_entries WHERE account_id = ?1 AND id NOT IN
               (SELECT id FROM undo_entries WHERE account_id = ?1 ORDER BY id DESC LIMIT ?2)",
            (account, UNDO_KEPT),
        )?;
        self.tx.touch(account, Table::Undo);
        Ok(())
    }

    /// Removes an entry, as undoing it does. Unknown ids are skipped.
    pub fn delete(&mut self, id: UndoEntryId) -> Result<(), StoreError> {
        let account: Option<AccountId> = self.tx.db().optional(
            "remove an undo entry",
            "DELETE FROM undo_entries WHERE id = ?1 RETURNING account_id",
            [id],
            |r| r.get("account_id"),
        )?;
        if let Some(account) = account {
            self.tx.touch(account, Table::Undo);
        }
        Ok(())
    }

    /// Removes a send's entry, once it is sent and its toast is gone.
    pub fn delete_for_outbox(&mut self, outbox: OutboxId) -> Result<(), StoreError> {
        let accounts: Vec<AccountId> = self.tx.db().all(
            "remove a send's undo entry",
            "DELETE FROM undo_entries WHERE outbox_id = ?1 RETURNING account_id",
            [outbox],
            |r| r.get("account_id"),
        )?;
        for account in accounts {
            self.tx.touch(account, Table::Undo);
        }
        Ok(())
    }
}
