use std::collections::BTreeSet;

use rusqlite::Row;
use sm_domain::{
    AccountId, GmailThreadId, HistoryId, LabelSet, MessageId, Millis, Rev, SplitId, SystemLabel,
    ThreadId, View,
};

use crate::db::{Db, id_list, json_column};
use crate::drafts::{DraftReads, DraftRecord};
use crate::messages::{self, AttachmentRecord, MessageReads, MessageRecord, NewMessage};
use crate::outbox::{OutboxReads, OutboxRecord};
use crate::routing;
use crate::tx::{WriteTx, require};
use crate::{StoreError, Table};

#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent derived flags of one thread"
)]
pub struct ThreadRow {
    pub id: ThreadId,
    pub account: AccountId,
    pub gmail_id: GmailThreadId,
    pub subject: String,
    pub snippet: String,
    pub sender: String,
    pub sender_email: String,
    pub sort_time: Millis,
    pub message_count: u32,
    pub unread: bool,
    pub has_attachments: bool,
    pub has_live: bool,
    pub in_inbox: bool,
    pub labels: LabelSet,
    pub snooze_until: Option<Millis>,
    pub split: Option<SplitId>,
    pub history_id: Option<HistoryId>,
    pub rev: Rev,
}

impl ThreadRow {
    #[must_use]
    pub fn starred(&self) -> bool {
        self.labels.has(SystemLabel::Starred)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListedThread {
    pub thread: ThreadId,
    pub split: Option<SplitId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSyncState {
    pub id: ThreadId,
    pub gmail_id: GmailThreadId,
    pub history_id: Option<HistoryId>,
    pub rev: Rev,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationMessage {
    pub message: MessageRecord,
    pub attachments: Vec<AttachmentRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationRows {
    pub thread: ThreadRow,
    pub messages: Vec<ConversationMessage>,
    pub drafts: Vec<DraftRecord>,
    pub outbox: Vec<OutboxRecord>,
}

fn thread_row(r: &Row<'_>) -> rusqlite::Result<ThreadRow> {
    Ok(ThreadRow {
        id: r.get("id")?,
        account: r.get("account_id")?,
        gmail_id: r.get("gmail_id")?,
        subject: r.get("subject")?,
        snippet: r.get("snippet")?,
        sender: r.get("sender")?,
        sender_email: r.get("sender_email")?,
        sort_time: r.get("sort_time")?,
        message_count: r.get("message_count")?,
        unread: r.get("unread")?,
        has_attachments: r.get("has_attachments")?,
        has_live: r.get("has_live")?,
        in_inbox: r.get("in_inbox")?,
        labels: json_column(r, "labels")?,
        snooze_until: r.get("snooze_until")?,
        split: r.get("split_id")?,
        history_id: r.get("history_id")?,
        rev: r.get("rev")?,
    })
}

fn listed(r: &Row<'_>) -> rusqlite::Result<ListedThread> {
    Ok(ListedThread {
        thread: r.get("id")?,
        split: r.get("split_id")?,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct ThreadReads<'c> {
    db: Db<'c>,
}

impl<'c> ThreadReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    pub fn row(self, id: ThreadId) -> Result<Option<ThreadRow>, StoreError> {
        self.db.optional(
            "read a thread",
            "SELECT t.id, t.account_id, t.gmail_id, t.subject, t.snippet, t.sender,
                    t.sender_email, t.sort_time, t.message_count, t.unread, t.has_attachments,
                    t.has_live, t.in_inbox, t.snooze_until, t.split_id, t.history_id, t.rev,
                    (SELECT json_group_array(label_id) FROM thread_labels
                     WHERE thread_id = t.id) AS labels
             FROM threads t WHERE t.id = ?1",
            [id],
            thread_row,
        )
    }

    pub fn rows(self, ids: &[ThreadId]) -> Result<Vec<ThreadRow>, StoreError> {
        self.db.all(
            "read threads",
            "SELECT t.id, t.account_id, t.gmail_id, t.subject, t.snippet, t.sender,
                    t.sender_email, t.sort_time, t.message_count, t.unread, t.has_attachments,
                    t.has_live, t.in_inbox, t.snooze_until, t.split_id, t.history_id, t.rev,
                    (SELECT json_group_array(label_id) FROM thread_labels
                     WHERE thread_id = t.id) AS labels
             FROM json_each(?1) j JOIN threads t ON t.id = j.value ORDER BY j.key",
            [id_list(ids)?],
            thread_row,
        )
    }

    pub fn list(self, account: AccountId, view: &View) -> Result<Vec<ListedThread>, StoreError> {
        let db = self.db;
        match view {
            View::Inbox => db.all(
                "list the Inbox",
                "SELECT t.id, t.split_id FROM threads t LEFT JOIN splits s ON s.id = t.split_id
                 WHERE t.account_id = ?1 AND t.in_inbox = 1 AND t.snooze_until IS NULL
                 ORDER BY s.position, t.sort_time DESC, t.id DESC",
                [account],
                listed,
            ),
            View::Done => db.all(
                "list Done",
                "SELECT t.id, NULL AS split_id FROM threads t
                 WHERE t.account_id = ?1 AND t.in_inbox = 0 AND t.snooze_until IS NULL
                   AND t.has_live = 1
                 ORDER BY t.sort_time DESC, t.id DESC",
                [account],
                listed,
            ),
            View::AllMail => db.all(
                "list All Mail",
                "SELECT t.id, NULL AS split_id FROM threads t
                 WHERE t.account_id = ?1 AND t.has_live = 1
                 ORDER BY t.sort_time DESC, t.id DESC",
                [account],
                listed,
            ),
            View::Snoozed => db.all(
                "list Snoozed",
                "SELECT t.id, NULL AS split_id FROM threads t
                 WHERE t.account_id = ?1 AND t.snooze_until IS NOT NULL
                 ORDER BY t.snooze_until, t.id",
                [account],
                listed,
            ),
            View::Drafts => Ok(Vec::new()),
            View::Starred | View::Sent | View::Trash | View::Spam | View::Label { .. } => {
                let label = view.label().unwrap_or_default();
                db.all(
                    "list a label view",
                    "SELECT t.id, NULL AS split_id
                     FROM thread_labels tl JOIN threads t ON t.id = tl.thread_id
                     WHERE tl.account_id = ?1 AND tl.label_id = ?2
                     ORDER BY t.sort_time DESC, t.id DESC",
                    (account, label),
                    listed,
                )
            }
        }
    }

    pub fn by_gmail_id(
        self,
        account: AccountId,
        gmail_id: &GmailThreadId,
    ) -> Result<Option<ThreadId>, StoreError> {
        self.db.optional(
            "find a thread by Gmail id",
            "SELECT id FROM threads WHERE account_id = ?1 AND gmail_id = ?2",
            (account, gmail_id),
            |r| r.get("id"),
        )
    }

    pub fn account_of(self, id: ThreadId) -> Result<Option<AccountId>, StoreError> {
        self.db.optional(
            "find a thread's account",
            "SELECT account_id FROM threads WHERE id = ?1",
            [id],
            |r| r.get("account_id"),
        )
    }

    pub(crate) fn inbox_splits(
        self,
        account: AccountId,
    ) -> Result<Vec<(ThreadId, Option<SplitId>)>, StoreError> {
        self.db.all(
            "read the Inbox splits",
            "SELECT id, split_id FROM threads WHERE account_id = ?1 AND in_inbox = 1",
            [account],
            |r| Ok((r.get("id")?, r.get("split_id")?)),
        )
    }

    pub fn sender_inbox_threads(
        self,
        account: AccountId,
        thread: ThreadId,
    ) -> Result<Vec<ThreadId>, StoreError> {
        self.db.all(
            "find a sender's Inbox threads",
            "WITH sender AS (
               SELECT lower(from_email) AS email FROM messages
               WHERE thread_id = ?2 AND is_draft = 0 AND from_email <> ''
               ORDER BY internal_date DESC, id DESC LIMIT 1)
             SELECT DISTINCT t.id FROM threads t JOIN messages m ON m.thread_id = t.id
             WHERE t.account_id = ?1 AND t.in_inbox = 1 AND m.is_draft = 0
               AND lower(m.from_email) = (SELECT email FROM sender)
             ORDER BY t.id",
            (account, thread),
            |r| r.get("id"),
        )
    }

    pub fn snoozes_due(self, now: Millis) -> Result<Vec<(AccountId, ThreadId)>, StoreError> {
        self.db.all(
            "read the due snoozes",
            "SELECT account_id, id FROM threads
             WHERE snooze_until IS NOT NULL AND snooze_until <= ?1
             ORDER BY snooze_until, id",
            [now],
            |r| Ok((r.get("account_id")?, r.get("id")?)),
        )
    }

    pub fn next_snooze(self) -> Result<Option<Millis>, StoreError> {
        self.db.one(
            "read the next snooze",
            "SELECT min(snooze_until) AS next FROM threads WHERE snooze_until IS NOT NULL",
            [],
            |r| r.get("next"),
        )
    }

    pub fn owed_reads(self, account: AccountId) -> Result<Vec<GmailThreadId>, StoreError> {
        self.db.all(
            "read the threads owed a read",
            "SELECT gmail_id FROM threads WHERE account_id = ?1 AND history_id IS NULL ORDER BY id",
            [account],
            |r| r.get("gmail_id"),
        )
    }

    pub fn sync_states(self, account: AccountId) -> Result<Vec<ThreadSyncState>, StoreError> {
        self.db.all(
            "read the threads' sync state",
            "SELECT id, gmail_id, history_id, rev FROM threads WHERE account_id = ?1 ORDER BY id",
            [account],
            |r| {
                Ok(ThreadSyncState {
                    id: r.get("id")?,
                    gmail_id: r.get("gmail_id")?,
                    history_id: r.get("history_id")?,
                    rev: r.get("rev")?,
                })
            },
        )
    }

    pub fn conversation(
        self,
        id: ThreadId,
        trash_view: bool,
    ) -> Result<Option<ConversationRows>, StoreError> {
        let Some(thread) = self.row(id)? else {
            return Ok(None);
        };
        let reads = MessageReads::new(self.db);
        let mut messages = Vec::new();
        for message in reads.for_thread(id)? {
            let trashed =
                message.labels.has(SystemLabel::Trash) || message.labels.has(SystemLabel::Spam);
            if message.is_draft || (trashed && !trash_view) {
                continue;
            }
            let attachments = reads.attachments(message.id)?;
            messages.push(ConversationMessage {
                message,
                attachments,
            });
        }
        Ok(Some(ConversationRows {
            thread,
            messages,
            drafts: DraftReads::new(self.db).on_thread(id)?,
            outbox: OutboxReads::new(self.db).for_thread(id)?,
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedThread {
    pub account: AccountId,
    pub gmail_id: GmailThreadId,
    pub history_id: Option<HistoryId>,
    pub messages: Vec<NewMessage>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredThread {
    pub thread: Option<ThreadId>,
    pub added: Vec<MessageId>,
    pub removed: Vec<MessageId>,
}

fn place_thread(db: Db<'_>, fetched: &FetchedThread, rev: Rev) -> Result<ThreadId, StoreError> {
    let account = fetched.account;
    db.execute(
        "add a thread",
        "INSERT INTO threads (account_id, gmail_id, rev) VALUES (?1, ?2, ?3)
         ON CONFLICT (account_id, gmail_id) DO NOTHING",
        (account, &fetched.gmail_id, rev),
    )?;
    let thread: ThreadId = db.one(
        "find a thread by Gmail id",
        "SELECT id FROM threads WHERE account_id = ?1 AND gmail_id = ?2",
        (account, &fetched.gmail_id),
        |r| r.get("id"),
    )?;
    if let Some(history) = fetched.history_id {
        db.execute(
            "store a thread's history id",
            "UPDATE threads SET history_id = ?2 WHERE id = ?1",
            (thread, history),
        )?;
    }
    Ok(thread)
}

#[derive(Debug)]
pub struct ThreadWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> ThreadWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    pub fn store(&mut self, fetched: &FetchedThread) -> Result<StoredThread, StoreError> {
        let account = fetched.account;
        let rev = self.tx.stamp();
        let db = self.tx.db();
        let thread = place_thread(db, fetched, rev)?;
        let before: Vec<MessageId> = db.all(
            "read a thread's message ids",
            "SELECT id FROM messages WHERE thread_id = ?1",
            [thread],
            |r| r.get("id"),
        )?;
        let mut touched = BTreeSet::from([thread]);
        let mut kept = BTreeSet::new();
        let mut added = Vec::new();
        for message in &fetched.messages {
            let (id, is_new) =
                messages::upsert_row(self.tx, account, thread, message, &mut touched)?;
            kept.insert(id);
            if is_new {
                added.push(id);
            }
        }
        let mut removed = Vec::new();
        if fetched.complete {
            for id in before.into_iter().filter(|id| !kept.contains(id)) {
                if messages::delete_row(self.tx, id)?.is_some() {
                    removed.push(id);
                }
            }
        }
        messages::derive_all(self.tx, touched)?;
        let thread = self.tx.read().threads().account_of(thread)?.map(|_| thread);
        Ok(StoredThread {
            thread,
            added,
            removed,
        })
    }

    pub fn set_snooze(&mut self, id: ThreadId, until: Option<Millis>) -> Result<(), StoreError> {
        let account = require(self.tx.read().threads().account_of(id)?, "thread", &id)?;
        let rev = self.tx.stamp();
        self.tx.db().execute(
            "store a thread's snooze",
            "UPDATE threads SET snooze_until = ?2, rev = ?3 WHERE id = ?1",
            (id, until, rev),
        )?;
        self.tx.record().thread(account, id);
        Ok(())
    }

    pub fn clear_history_ids(&self, ids: &[ThreadId]) -> Result<(), StoreError> {
        for &id in ids {
            self.tx.db().execute(
                "mark a thread owed a read",
                "UPDATE threads SET history_id = NULL WHERE id = ?1",
                [id],
            )?;
        }
        Ok(())
    }

    pub fn set_history_id(
        &mut self,
        id: ThreadId,
        history_id: HistoryId,
    ) -> Result<(), StoreError> {
        let account: Option<AccountId> = self.tx.db().optional(
            "store a thread's history id",
            "UPDATE threads SET history_id = ?2 WHERE id = ?1 RETURNING account_id",
            (id, history_id),
            |r| r.get("account_id"),
        )?;
        if let Some(account) = account {
            self.tx.touch(account, Table::SyncCursor);
        }
        Ok(())
    }

    pub fn delete(&mut self, id: ThreadId) -> Result<(), StoreError> {
        let Some(account) = self.tx.read().threads().account_of(id)? else {
            return Ok(());
        };
        let ids: Vec<MessageId> = self.tx.db().all(
            "read a thread's message ids",
            "SELECT id FROM messages WHERE thread_id = ?1",
            [id],
            |r| r.get("id"),
        )?;
        for message in ids {
            messages::delete_row(self.tx, message)?;
        }
        routing::delete_thread_row(self.tx, account, id)
    }
}
