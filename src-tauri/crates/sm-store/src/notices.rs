use rusqlite::{Row, named_params};
use sm_domain::{AccountId, DraftId, Millis, NoticeId, NoticeKind, ThreadId};

use crate::db::Db;
use crate::tx::WriteTx;
use crate::{StoreError, Table};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNotice {
    pub account: AccountId,
    pub kind: NoticeKind,
    pub message: String,
    pub draft: Option<DraftId>,
    pub thread: Option<ThreadId>,
    pub created_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeRecord {
    pub id: NoticeId,
    pub account: AccountId,
    pub kind: NoticeKind,
    pub message: String,
    pub draft: Option<DraftId>,
    pub thread: Option<ThreadId>,
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

#[derive(Debug, Clone, Copy)]
pub struct NoticeReads<'c> {
    db: Db<'c>,
}

impl<'c> NoticeReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    pub fn for_account(self, account: AccountId) -> Result<Vec<NoticeRecord>, StoreError> {
        self.db.all(
            "read the notices",
            "SELECT id, account_id, kind, message, draft_id, thread_id, created_at
             FROM notices WHERE account_id = ?1 ORDER BY id",
            [account],
            notice_row,
        )
    }
}

#[derive(Debug)]
pub struct NoticeWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> NoticeWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

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
