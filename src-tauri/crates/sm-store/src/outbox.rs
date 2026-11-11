//! The outbox: sends in their undo window, at Gmail, or done (SPEC Send and
//! undo).

use rusqlite::{Row, named_params};
use sm_domain::{
    AccountId, Addr, DraftId, GmailDraftId, GmailMessageId, GmailThreadId, Millis, OutboxId,
    OutboxState, Rev, ThreadId, UndoEntryId,
};

use crate::StoreError;
use crate::db::{Db, json_column_opt};
use crate::drafts::stamp_thread;
use crate::tx::{WriteTx, require};

/// An outbox row's state. Read it, change it, and write it back with
/// `OutboxWrites::update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outbox {
    /// Its id, never reused.
    pub id: OutboxId,
    /// Its account.
    pub account: AccountId,
    /// The draft it sends.
    pub draft: DraftId,
    /// The thread it replies on, or once sent, the thread it landed on.
    pub thread: Option<ThreadId>,
    /// When the undo window ends.
    pub send_after: Millis,
    /// Waiting, sending, or sent.
    pub state: OutboxState,
    /// The Gmail draft the send wrote.
    pub gmail_draft_id: Option<GmailDraftId>,
    /// The sent message's Gmail id.
    pub sent_message_id: Option<GmailMessageId>,
    /// A held undo, applied when the send resolves.
    pub undo_requested: bool,
    /// The toast was closed.
    pub toast_closed: bool,
    /// It missed its time: the toast says `Email delayed`.
    pub delayed: bool,
    /// Gmail's refusal, if it refused.
    pub error: Option<String>,
}

/// A stored outbox row, with what the toast and the pending message show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRecord {
    /// Its state.
    pub outbox: Outbox,
    /// The `rev` of its last change.
    pub rev: Rev,
    /// The draft's `To`; empty when the draft is gone.
    pub to: Vec<Addr>,
    /// The draft's subject.
    pub subject: String,
    /// The draft's body as text.
    pub snippet: String,
    /// The send's undo entry, while it is in the history.
    pub undo_entry: Option<UndoEntryId>,
}

/// A send to add, waiting out its undo window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOutbox {
    /// Its account.
    pub account: AccountId,
    /// The draft it sends.
    pub draft: DraftId,
    /// The thread it replies on.
    pub thread: Option<ThreadId>,
    /// When the undo window ends.
    pub send_after: Millis,
}

fn outbox_row(r: &Row<'_>) -> rusqlite::Result<OutboxRecord> {
    Ok(OutboxRecord {
        outbox: Outbox {
            id: r.get("id")?,
            account: r.get("account_id")?,
            draft: r.get("draft_id")?,
            thread: r.get("thread_id")?,
            send_after: r.get("send_after")?,
            state: r.get("state")?,
            gmail_draft_id: r.get("gmail_draft_id")?,
            sent_message_id: r.get("sent_message_id")?,
            undo_requested: r.get("undo_requested")?,
            toast_closed: r.get("toast_closed")?,
            delayed: r.get("delayed")?,
            error: r.get("error")?,
        },
        rev: r.get("rev")?,
        to: json_column_opt(r, "to_addrs")?.unwrap_or_default(),
        subject: r.get::<_, Option<String>>("subject")?.unwrap_or_default(),
        snippet: r.get::<_, Option<String>>("snippet")?.unwrap_or_default(),
        undo_entry: r.get("undo_entry")?,
    })
}

/// Reads of the outbox.
#[derive(Debug, Clone, Copy)]
pub struct OutboxReads<'c> {
    db: Db<'c>,
}

impl<'c> OutboxReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// One row.
    pub fn get(self, id: OutboxId) -> Result<Option<OutboxRecord>, StoreError> {
        self.db.optional(
            "read an outbox row",
            "SELECT o.*, d.to_addrs, d.subject, d.snippet,
                    (SELECT u.id FROM undo_entries u WHERE u.outbox_id = o.id) AS undo_entry
             FROM outbox o LEFT JOIN drafts d ON d.id = o.draft_id WHERE o.id = ?1",
            [id],
            outbox_row,
        )
    }

    /// The account's rows, oldest first: the send toasts.
    pub fn for_account(self, account: AccountId) -> Result<Vec<OutboxRecord>, StoreError> {
        self.db.all(
            "read an account's outbox",
            "SELECT o.*, d.to_addrs, d.subject, d.snippet,
                    (SELECT u.id FROM undo_entries u WHERE u.outbox_id = o.id) AS undo_entry
             FROM outbox o LEFT JOIN drafts d ON d.id = o.draft_id
             WHERE o.account_id = ?1 ORDER BY o.id",
            [account],
            outbox_row,
        )
    }

    /// A thread's rows, less sent ones whose message the thread now holds:
    /// the conversation shows the message from then on.
    pub fn for_thread(self, thread: ThreadId) -> Result<Vec<OutboxRecord>, StoreError> {
        self.db.all(
            "read a thread's outbox",
            "SELECT o.*, d.to_addrs, d.subject, d.snippet,
                    (SELECT u.id FROM undo_entries u WHERE u.outbox_id = o.id) AS undo_entry
             FROM outbox o LEFT JOIN drafts d ON d.id = o.draft_id
             WHERE o.thread_id = ?1
               AND NOT (o.state = 'sent' AND EXISTS (
                 SELECT 1 FROM messages m
                 WHERE m.thread_id = o.thread_id AND m.gmail_id = o.sent_message_id))
             ORDER BY o.id",
            [thread],
            outbox_row,
        )
    }

    /// Rows not yet sent, by send time: the outbox runner's work.
    pub fn unsent(self, account: AccountId) -> Result<Vec<OutboxRecord>, StoreError> {
        self.db.all(
            "read the unsent outbox",
            "SELECT o.*, d.to_addrs, d.subject, d.snippet,
                    (SELECT u.id FROM undo_entries u WHERE u.outbox_id = o.id) AS undo_entry
             FROM outbox o LEFT JOIN drafts d ON d.id = o.draft_id
             WHERE o.account_id = ?1 AND o.state != 'sent'
             ORDER BY o.send_after, o.id",
            [account],
            outbox_row,
        )
    }

    /// Sent rows whose toast is closed and whose message is stored, or that
    /// have no message to wait for (no thread, or no message id): done, so
    /// the row and its draft can go.
    pub fn finished(self, account: AccountId) -> Result<Vec<(OutboxId, DraftId)>, StoreError> {
        self.db.all(
            "read the finished outbox",
            "SELECT o.id, o.draft_id FROM outbox o
             WHERE o.account_id = ?1 AND o.state = 'sent' AND o.toast_closed = 1
               AND (o.thread_id IS NULL OR o.sent_message_id IS NULL OR EXISTS (
                 SELECT 1 FROM messages m
                 WHERE m.account_id = o.account_id AND m.gmail_id = o.sent_message_id))
             ORDER BY o.id",
            [account],
            |r| Ok((r.get("id")?, r.get("draft_id")?)),
        )
    }

    /// The Gmail thread a send goes to: its thread's, else its draft's.
    pub fn gmail_thread(self, id: OutboxId) -> Result<Option<GmailThreadId>, StoreError> {
        Ok(self
            .db
            .optional(
                "find a send's Gmail thread",
                "SELECT coalesce(t.gmail_id, d.gmail_thread_id) AS gmail_thread
                 FROM outbox o
                 LEFT JOIN drafts d ON d.id = o.draft_id
                 LEFT JOIN threads t ON t.id = o.thread_id
                 WHERE o.id = ?1",
                [id],
                |r| r.get::<_, Option<GmailThreadId>>("gmail_thread"),
            )?
            .flatten())
    }

    /// Gmail draft ids that sends hold, so draft sync leaves them alone.
    pub fn gmail_draft_ids(self, account: AccountId) -> Result<Vec<GmailDraftId>, StoreError> {
        self.db.all(
            "read the outbox's Gmail drafts",
            "SELECT gmail_draft_id FROM outbox
             WHERE account_id = ?1 AND gmail_draft_id IS NOT NULL ORDER BY id",
            [account],
            |r| r.get("gmail_draft_id"),
        )
    }
}

/// Writes to the outbox. Each stamps the row and its thread.
#[derive(Debug)]
pub struct OutboxWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> OutboxWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Adds a waiting send. Its id is never reused.
    pub fn insert(&mut self, new: NewOutbox) -> Result<OutboxId, StoreError> {
        let rev = self.tx.stamp();
        let id = OutboxId(self.tx.db().insert(
            "add a send",
            "INSERT INTO outbox (account_id, draft_id, thread_id, send_after, state, rev)
             VALUES (:account, :draft, :thread, :send_after, 'waiting', :rev)",
            named_params! {
                ":account": new.account, ":draft": new.draft, ":thread": new.thread,
                ":send_after": new.send_after, ":rev": rev,
            },
        )?);
        self.changed(new.account, id, [new.thread, None])?;
        // The draft is in the outbox now.
        self.tx.record().drafts.insert(new.draft);
        Ok(id)
    }

    /// Writes a row's state back. Its account and draft can't change.
    pub fn update(&mut self, row: &Outbox) -> Result<(), StoreError> {
        let (account, draft, thread) = require(self.placed(row.id)?, "outbox row", &row.id)?;
        if account != row.account || draft != row.draft {
            return Err(StoreError::Invalid(format!(
                "outbox row {} can't change its account or draft",
                row.id
            )));
        }
        let rev = self.tx.stamp();
        self.tx.db().execute(
            "store an outbox row",
            "UPDATE outbox SET thread_id = :thread, send_after = :send_after, state = :state,
                 gmail_draft_id = :gmail_draft, sent_message_id = :sent,
                 undo_requested = :undo, toast_closed = :closed, delayed = :delayed,
                 error = :error, rev = :rev
             WHERE id = :id",
            named_params! {
                ":id": row.id, ":thread": row.thread, ":send_after": row.send_after,
                ":state": row.state, ":gmail_draft": row.gmail_draft_id,
                ":sent": row.sent_message_id, ":undo": row.undo_requested,
                ":closed": row.toast_closed, ":delayed": row.delayed, ":error": row.error,
                ":rev": rev,
            },
        )?;
        self.changed(row.account, row.id, [thread, row.thread])
    }

    /// Deletes a row. Unknown ids are skipped.
    pub fn delete(&mut self, id: OutboxId) -> Result<(), StoreError> {
        let Some((account, draft, thread)) = self.placed(id)? else {
            return Ok(());
        };
        self.tx.stamp();
        self.tx.db().execute(
            "delete an outbox row",
            "DELETE FROM outbox WHERE id = ?1",
            [id],
        )?;
        self.changed(account, id, [thread, None])?;
        // The draft, if kept, is out of the outbox.
        self.tx.record().drafts.insert(draft);
        Ok(())
    }

    /// The account, draft, and thread of a stored row.
    fn placed(
        &self,
        id: OutboxId,
    ) -> Result<Option<(AccountId, DraftId, Option<ThreadId>)>, StoreError> {
        self.tx.db().optional(
            "read where an outbox row is",
            "SELECT account_id, draft_id, thread_id FROM outbox WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get("account_id")?,
                    r.get("draft_id")?,
                    r.get("thread_id")?,
                ))
            },
        )
    }

    fn changed(
        &mut self,
        account: AccountId,
        id: OutboxId,
        threads: [Option<ThreadId>; 2],
    ) -> Result<(), StoreError> {
        self.tx.record().outbox(account, id);
        for thread in threads.into_iter().flatten() {
            stamp_thread(self.tx, thread)?;
        }
        Ok(())
    }
}
