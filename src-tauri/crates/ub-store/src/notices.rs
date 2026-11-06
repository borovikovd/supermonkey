//! Notices waiting for the UI to show and ack them (SPEC UI state).

use rusqlite::{Row, named_params};
use ub_domain::{AccountId, DraftId, Millis, NoticeId, NoticeKind, ThreadId};

use crate::db::Db;
use crate::tx::WriteTx;
use crate::{StoreError, Table};

/// A notice to add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNotice {
    /// Its account.
    pub account: AccountId,
    /// Why it is shown.
    pub kind: NoticeKind,
    /// What the toast says.
    pub message: String,
    /// The draft it is about, for `Open draft`.
    pub draft: Option<DraftId>,
    /// The thread it is about.
    pub thread: Option<ThreadId>,
    /// When it was made.
    pub created_at: Millis,
}

/// A stored notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeRecord {
    /// Its id, never reused.
    pub id: NoticeId,
    /// Its account.
    pub account: AccountId,
    /// Why it is shown.
    pub kind: NoticeKind,
    /// What the toast says.
    pub message: String,
    /// The draft it is about.
    pub draft: Option<DraftId>,
    /// The thread it is about.
    pub thread: Option<ThreadId>,
    /// When it was made.
    pub created_at: Millis,
}

fn notice_row(r: &Row<'_>) -> rusqlite::Result<NoticeRecord> {
    Ok(NoticeRecord {
        id: r.get("id")?,
        account: r.get("account_id")?,
        kind: r.get("kind")?,
        message: r.get("message")?,
        draft: r.get("draft_id")?,
        thread: r.get("thread_id")?,
        created_at: r.get("created_at")?,
    })
}

/// Reads of notices.
#[derive(Clone, Copy)]
pub struct NoticeReads<'c> {
    db: Db<'c>,
}

impl<'c> NoticeReads<'c> {
    pub(crate) fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// The account's notices, oldest first.
    pub fn for_account(self, account: AccountId) -> Result<Vec<NoticeRecord>, StoreError> {
        self.db.all(
            "read the notices",
            "SELECT id, account_id, kind, message, draft_id, thread_id, created_at
             FROM notices WHERE account_id = ?1 ORDER BY id",
            [account],
            notice_row,
        )
    }

    /// One notice.
    pub fn get(self, id: NoticeId) -> Result<Option<NoticeRecord>, StoreError> {
        self.db.optional(
            "read a notice",
            "SELECT id, account_id, kind, message, draft_id, thread_id, created_at
             FROM notices WHERE id = ?1",
            [id],
            notice_row,
        )
    }
}

/// Writes to notices.
pub struct NoticeWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> NoticeWrites<'t, 'c> {
    pub(crate) fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Adds a notice. Its id is never reused.
    pub fn add(&mut self, notice: &NewNotice) -> Result<NoticeId, StoreError> {
        let id = NoticeId(self.tx.db().insert(
            "add a notice",
            "INSERT INTO notices (account_id, kind, message, draft_id, thread_id, created_at)
             VALUES (:account, :kind, :message, :draft, :thread, :created_at)",
            named_params! {
                ":account": notice.account, ":kind": notice.kind, ":message": notice.message,
                ":draft": notice.draft, ":thread": notice.thread,
                ":created_at": notice.created_at,
            },
        )?);
        self.tx.touch(notice.account, Table::Notices);
        Ok(id)
    }

    /// Removes a notice the UI acked. Unknown ids are skipped.
    pub fn delete(&mut self, id: NoticeId) -> Result<(), StoreError> {
        let account: Option<AccountId> = self.tx.db().optional(
            "remove a notice",
            "DELETE FROM notices WHERE id = ?1 RETURNING account_id",
            [id],
            |r| r.get("account_id"),
        )?;
        if let Some(account) = account {
            self.tx.touch(account, Table::Notices);
        }
        Ok(())
    }
}
