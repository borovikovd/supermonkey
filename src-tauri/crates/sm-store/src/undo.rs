use rusqlite::{Row, named_params};
use serde::{Deserialize, Serialize};
use sm_domain::{
    AccountId, OutboxId, PinKind, SplitId, ThreadChange, ThreadId, UndoEntryId, UndoKind,
};

use crate::db::{Db, json_column, to_json};
use crate::tx::WriteTx;
use crate::{StoreError, Table};

pub const UNDO_KEPT: u32 = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UndoAction {
    Mail {
        threads: Vec<ThreadChange>,
    },
    Send {
        outbox: OutboxId,
    },
    Pin {
        pin_kind: PinKind,
        value: String,
        split: SplitId,
        replaced: Option<SplitId>,
    },
}

impl UndoAction {
    #[must_use]
    pub const fn kind(&self) -> UndoKind {
        match self {
            Self::Mail { .. } => UndoKind::Mail,
            Self::Send { .. } => UndoKind::Send,
            Self::Pin { .. } => UndoKind::Pin,
        }
    }

    #[must_use]
    pub fn threads(&self) -> Vec<ThreadId> {
        match self {
            Self::Mail { threads } => threads.iter().map(|t| t.thread).collect(),
            Self::Send { .. } | Self::Pin { .. } => Vec::new(),
        }
    }

    #[must_use]
    pub const fn outbox(&self) -> Option<OutboxId> {
        match self {
            Self::Send { outbox } => Some(*outbox),
            Self::Mail { .. } | Self::Pin { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoRecord {
    pub id: UndoEntryId,
    pub account: AccountId,
    pub label: String,
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

#[derive(Debug, Clone, Copy)]
pub struct UndoReads<'c> {
    db: Db<'c>,
}

impl<'c> UndoReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    pub fn entries(self, account: AccountId) -> Result<Vec<UndoRecord>, StoreError> {
        self.db.all(
            "read the undo history",
            "SELECT id, account_id, label, payload FROM undo_entries
             WHERE account_id = ?1 ORDER BY id",
            [account],
            undo_row,
        )
    }

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

#[derive(Debug)]
pub struct UndoWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> UndoWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

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
