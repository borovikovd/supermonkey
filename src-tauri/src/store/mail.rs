use std::collections::{HashMap, HashSet};

use rusqlite::{Row, params};

use crate::domain::{
    AccountId, Addr, AttachmentId, DraftId, GmailAttachmentId, GmailMessageId, GmailThreadId,
    HistoryId, LabelId, LabelSet, MessageFacts, MessageId, Millis, Rev, Router, SplitId,
    SystemLabel, Thread, ThreadId, View,
};

use super::sql::{id_list, json_col, to_json};
use super::{CachedRouting, Db, StoreError, Table, WriteTx};

const THREAD_ROW_COLS: &str = "t.id, t.account_id, t.gmail_id, t.subject, t.snippet, t.sender, t.sender_email, t.sort_time, t.message_count, t.unread, t.has_attachments, t.has_live, t.in_inbox, t.snooze_until, t.split_id, t.history_id, t.rev";

const MSG_COLS: &str = "id, account_id, thread_id, gmail_id, internal_date, from_name, from_email, to_addrs, cc_addrs, bcc_addrs, reply_to, subject, snippet, message_id_hdr, in_reply_to, references_hdr, is_list, is_draft, is_live, sm_draft_id, label_ids, has_attachments, has_body, rev";

const ATT_COLS: &str = "id, message_id, part_id, filename, mime_type, size, gmail_att_id, content_id, is_inline, local_path";

#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors independent boolean columns in the threads table"
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
    pub snooze_until: Option<Millis>,
    pub split: Option<SplitId>,
    pub history_id: Option<HistoryId>,
    pub rev: Rev,
    pub labels: Vec<LabelId>,
}

impl ThreadRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            account: row.get(1)?,
            gmail_id: row.get(2)?,
            subject: row.get(3)?,
            snippet: row.get(4)?,
            sender: row.get(5)?,
            sender_email: row.get(6)?,
            sort_time: row.get(7)?,
            message_count: u32::try_from(row.get::<_, i64>(8)?.max(0)).unwrap_or(u32::MAX),
            unread: row.get::<_, i64>(9)? != 0,
            has_attachments: row.get::<_, i64>(10)? != 0,
            has_live: row.get::<_, i64>(11)? != 0,
            in_inbox: row.get::<_, i64>(12)? != 0,
            snooze_until: row.get(13)?,
            split: row.get(14)?,
            history_id: row.get(15)?,
            rev: row.get(16)?,
            labels: Vec::new(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSyncState {
    pub id: ThreadId,
    pub gmail_id: GmailThreadId,
    pub history_id: Option<HistoryId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors independent boolean columns in the messages table"
)]
pub struct MessageRecord {
    pub id: MessageId,
    pub account: AccountId,
    pub thread: ThreadId,
    pub gmail_id: GmailMessageId,
    pub date: Millis,
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub reply_to: Vec<Addr>,
    pub subject: String,
    pub snippet: String,
    pub message_id_hdr: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub is_list: bool,
    pub is_draft: bool,
    pub is_live: bool,
    pub sm_draft_id: Option<DraftId>,
    pub labels: LabelSet,
    pub has_attachments: bool,
    pub has_body: bool,
    pub rev: Rev,
}

impl MessageRecord {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let label_vec: Vec<LabelId> = json_col(row, 20)?;
        Ok(Self {
            id: row.get(0)?,
            account: row.get(1)?,
            thread: row.get(2)?,
            gmail_id: row.get(3)?,
            date: row.get(4)?,
            from: Addr {
                name: row.get(5)?,
                email: row.get(6)?,
            },
            to: json_col(row, 7)?,
            cc: json_col(row, 8)?,
            bcc: json_col(row, 9)?,
            reply_to: json_col(row, 10)?,
            subject: row.get(11)?,
            snippet: row.get(12)?,
            message_id_hdr: row.get(13)?,
            in_reply_to: row.get(14)?,
            references: json_col(row, 15)?,
            is_list: row.get::<_, i64>(16)? != 0,
            is_draft: row.get::<_, i64>(17)? != 0,
            is_live: row.get::<_, i64>(18)? != 0,
            sm_draft_id: row.get(19)?,
            labels: label_vec.into_iter().collect(),
            has_attachments: row.get::<_, i64>(21)? != 0,
            has_body: row.get::<_, i64>(22)? != 0,
            rev: row.get(23)?,
        })
    }

    #[must_use]
    pub fn into_facts(self) -> MessageFacts {
        MessageFacts {
            id: self.id,
            gmail_id: self.gmail_id,
            date: self.date,
            from: self.from,
            to: self.to,
            cc: self.cc,
            bcc: self.bcc,
            reply_to: self.reply_to,
            subject: self.subject,
            snippet: self.snippet,
            message_id_hdr: self.message_id_hdr,
            in_reply_to: self.in_reply_to,
            references: self.references,
            is_list: self.is_list,
            is_draft: self.is_draft,
            has_attachments: self.has_attachments,
            labels: self.labels,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRecord {
    pub id: AttachmentId,
    pub message: MessageId,
    pub part_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: i64,
    pub gmail_att_id: Option<GmailAttachmentId>,
    pub content_id: Option<String>,
    pub is_inline: bool,
    pub local_path: Option<String>,
}

impl AttachmentRecord {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            message: row.get(1)?,
            part_id: row.get(2)?,
            filename: row.get(3)?,
            mime_type: row.get(4)?,
            size: row.get(5)?,
            gmail_att_id: row.get(6)?,
            content_id: row.get(7)?,
            is_inline: row.get::<_, i64>(8)? != 0,
            local_path: row.get(9)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAttachment {
    pub part_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: i64,
    pub gmail_att_id: Option<GmailAttachmentId>,
    pub content_id: Option<String>,
    pub is_inline: bool,
    pub local_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewMessage {
    pub gmail_id: GmailMessageId,
    pub date: Millis,
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub reply_to: Vec<Addr>,
    pub subject: String,
    pub snippet: String,
    pub message_id_hdr: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub is_list: bool,
    pub is_draft: bool,
    pub sm_draft_id: Option<DraftId>,
    pub labels: LabelSet,
    pub has_attachments: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationView {
    pub thread: ThreadRow,
    pub messages: Vec<(MessageRecord, Vec<AttachmentRecord>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageBodyRecord {
    pub id: MessageId,
    pub html: String,
    pub quote_html: Option<String>,
}

impl Db<'_> {
    pub fn thread_row(self, id: ThreadId) -> Result<Option<ThreadRow>, StoreError> {
        let sql = format!("SELECT {THREAD_ROW_COLS} FROM threads t WHERE t.id = ?1");
        let Some(mut row) = self.optional(&sql, [id], "read thread row", ThreadRow::from_row)?
        else {
            return Ok(None);
        };
        row.labels = self.thread_labels_for(id)?;
        Ok(Some(row))
    }

    pub fn thread_rows(self, ids: &[ThreadId]) -> Result<Vec<ThreadRow>, StoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids_json = id_list(ids)?;
        let sql = format!(
            "SELECT {THREAD_ROW_COLS} FROM threads t WHERE t.id IN (SELECT value FROM json_each(?1))"
        );
        let mut rows = self.all(&sql, [&ids_json], "read thread rows", ThreadRow::from_row)?;
        self.attach_thread_labels(&mut rows, &ids_json)?;
        Ok(rows)
    }

    pub fn thread_by_gmail(
        self,
        account: AccountId,
        gmail_id: &GmailThreadId,
    ) -> Result<Option<ThreadId>, StoreError> {
        self.optional(
            "SELECT id FROM threads WHERE account_id = ?1 AND gmail_id = ?2",
            params![account, gmail_id],
            "find thread by gmail id",
            |r| r.get(0),
        )
    }

    pub fn thread_gmail_id(
        self,
        id: ThreadId,
    ) -> Result<Option<(AccountId, GmailThreadId)>, StoreError> {
        self.optional(
            "SELECT account_id, gmail_id FROM threads WHERE id = ?1",
            [id],
            "read thread gmail id",
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    }

    pub fn view_threads(
        self,
        account: AccountId,
        view: &View,
    ) -> Result<Vec<(ThreadId, Option<SplitId>)>, StoreError> {
        match view {
            View::Inbox => self.all(
                "SELECT t.id, t.split_id FROM threads t
                 LEFT JOIN splits s ON s.id = t.split_id
                 WHERE t.account_id = ?1 AND t.in_inbox = 1
                 ORDER BY COALESCE(s.position, 20000), t.sort_time DESC, t.id DESC",
                [account],
                "list inbox threads",
                |r| Ok((r.get(0)?, r.get(1)?)),
            ),
            View::Done => self.all(
                "SELECT id, NULL FROM threads WHERE account_id = ?1 AND has_live = 1 AND in_inbox = 0 AND snooze_until IS NULL ORDER BY sort_time DESC, id DESC",
                [account],
                "list done threads",
                |r| Ok((r.get(0)?, r.get(1)?)),
            ),
            View::AllMail => self.all(
                "SELECT id, NULL FROM threads WHERE account_id = ?1 AND has_live = 1 ORDER BY sort_time DESC, id DESC",
                [account],
                "list all mail threads",
                |r| Ok((r.get(0)?, r.get(1)?)),
            ),
            View::Snoozed => self.all(
                "SELECT id, NULL FROM threads WHERE account_id = ?1 AND has_live = 1 AND in_inbox = 0 AND snooze_until IS NOT NULL ORDER BY snooze_until ASC, id DESC",
                [account],
                "list snoozed threads",
                |r| Ok((r.get(0)?, r.get(1)?)),
            ),
            View::Starred => self.threads_with_label(account, SystemLabel::Starred.id(), true),
            View::Sent => self.threads_with_label(account, SystemLabel::Sent.id(), true),
            View::Trash => self.threads_with_label(account, SystemLabel::Trash.id(), false),
            View::Spam => self.threads_with_label(account, SystemLabel::Spam.id(), false),
            View::Label { label } => self.threads_with_label(account, label.as_str(), true),
            View::Drafts => Ok(Vec::new()),
        }
    }

    fn threads_with_label(
        self,
        account: AccountId,
        label: &str,
        live_only: bool,
    ) -> Result<Vec<(ThreadId, Option<SplitId>)>, StoreError> {
        let sql = if live_only {
            "SELECT t.id, NULL FROM threads t
             JOIN thread_labels tl ON tl.thread_id = t.id
             WHERE tl.account_id = ?1 AND tl.label_id = ?2 AND t.has_live = 1
             ORDER BY t.sort_time DESC, t.id DESC"
        } else {
            "SELECT t.id, NULL FROM threads t
             JOIN thread_labels tl ON tl.thread_id = t.id
             WHERE tl.account_id = ?1 AND tl.label_id = ?2
             ORDER BY t.sort_time DESC, t.id DESC"
        };
        self.all(sql, params![account, label], "list label threads", |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
    }

    pub fn conversation(
        self,
        id: ThreadId,
        trash_mode: bool,
    ) -> Result<Option<ConversationView>, StoreError> {
        let Some(thread) = self.thread_row(id)? else {
            return Ok(None);
        };
        let sql = if trash_mode {
            format!(
                "SELECT {MSG_COLS} FROM messages WHERE thread_id = ?1 AND is_draft = 0 ORDER BY internal_date, id"
            )
        } else {
            format!(
                "SELECT {MSG_COLS} FROM messages WHERE thread_id = ?1 AND is_live = 1 ORDER BY internal_date, id"
            )
        };
        let msgs = self.all(
            &sql,
            [id],
            "read conversation messages",
            MessageRecord::from_row,
        )?;
        let att_map = self.attachments_for_messages(&msgs)?;
        let messages = msgs
            .into_iter()
            .map(|m| {
                let atts = att_map.get(&m.id).cloned().unwrap_or_default();
                (m, atts)
            })
            .collect();
        Ok(Some(ConversationView { thread, messages }))
    }

    pub fn thread_messages(self, id: ThreadId) -> Result<Vec<MessageRecord>, StoreError> {
        let sql = format!(
            "SELECT {MSG_COLS} FROM messages WHERE thread_id = ?1 ORDER BY internal_date, id"
        );
        self.all(&sql, [id], "read thread messages", MessageRecord::from_row)
    }

    pub fn thread_facts(self, id: ThreadId) -> Result<Thread, StoreError> {
        let facts = self.load_message_facts(id)?;
        Ok(Thread::new(facts))
    }

    pub fn message(self, id: MessageId) -> Result<Option<MessageRecord>, StoreError> {
        let sql = format!("SELECT {MSG_COLS} FROM messages WHERE id = ?1");
        self.optional(&sql, [id], "read message", MessageRecord::from_row)
    }

    pub fn message_by_gmail(
        self,
        account: AccountId,
        gmail_id: &GmailMessageId,
    ) -> Result<Option<MessageRecord>, StoreError> {
        let sql =
            format!("SELECT {MSG_COLS} FROM messages WHERE account_id = ?1 AND gmail_id = ?2");
        self.optional(
            &sql,
            params![account, gmail_id],
            "read message by gmail id",
            MessageRecord::from_row,
        )
    }

    pub fn message_bodies(self, ids: &[MessageId]) -> Result<Vec<MessageBodyRecord>, StoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids_json = id_list(ids)?;
        self.all(
            "SELECT id, COALESCE(body_html, ''), quote_html FROM messages WHERE has_body = 1 AND id IN (SELECT value FROM json_each(?1))",
            [ids_json],
            "read message bodies",
            |r| {
                Ok(MessageBodyRecord {
                    id: r.get(0)?,
                    html: r.get(1)?,
                    quote_html: r.get(2)?,
                })
            },
        )
    }

    pub fn message_body(self, id: MessageId) -> Result<Option<MessageBodyRecord>, StoreError> {
        self.optional(
            "SELECT id, COALESCE(body_html, ''), quote_html FROM messages WHERE id = ?1 AND has_body = 1",
            [id],
            "read message body",
            |r| {
                Ok(MessageBodyRecord {
                    id: r.get(0)?,
                    html: r.get(1)?,
                    quote_html: r.get(2)?,
                })
            },
        )
    }

    pub fn attachment(
        self,
        id: AttachmentId,
    ) -> Result<Option<(AccountId, GmailMessageId, AttachmentRecord)>, StoreError> {
        const SQL: &str = "SELECT m.account_id, m.gmail_id, a.id, a.message_id, a.part_id, a.filename, a.mime_type, a.size, a.gmail_att_id, a.content_id, a.is_inline, a.local_path
             FROM attachments a JOIN messages m ON m.id = a.message_id WHERE a.id = ?1";
        self.optional(SQL, [id], "read attachment", |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                AttachmentRecord {
                    id: r.get(2)?,
                    message: r.get(3)?,
                    part_id: r.get(4)?,
                    filename: r.get(5)?,
                    mime_type: r.get(6)?,
                    size: r.get(7)?,
                    gmail_att_id: r.get(8)?,
                    content_id: r.get(9)?,
                    is_inline: r.get::<_, i64>(10)? != 0,
                    local_path: r.get(11)?,
                },
            ))
        })
    }

    pub fn attachments_for_message(
        self,
        id: MessageId,
    ) -> Result<Vec<AttachmentRecord>, StoreError> {
        let sql = format!("SELECT {ATT_COLS} FROM attachments WHERE message_id = ?1 ORDER BY id");
        self.all(
            &sql,
            [id],
            "read message attachments",
            AttachmentRecord::from_row,
        )
    }

    pub fn attachment_files(self, account: AccountId) -> Result<Vec<String>, StoreError> {
        self.all(
            "SELECT a.local_path FROM attachments a JOIN messages m ON m.id = a.message_id WHERE m.account_id = ?1 AND a.local_path IS NOT NULL",
            [account],
            "list account attachment files",
            |r| r.get(0),
        )
    }

    pub fn sender_inbox_threads(
        self,
        account: AccountId,
        email: &str,
    ) -> Result<Vec<ThreadId>, StoreError> {
        self.all(
            "SELECT id FROM threads WHERE account_id = ?1 AND in_inbox = 1 AND lower(sender_email) = lower(?2) ORDER BY sort_time DESC",
            params![account, email],
            "list sender inbox threads",
            |r| r.get(0),
        )
    }

    pub fn due_snoozes(self, now: Millis) -> Result<Vec<(AccountId, ThreadId)>, StoreError> {
        self.all(
            "SELECT account_id, id FROM threads WHERE snooze_until IS NOT NULL AND snooze_until <= ?1",
            [now],
            "find due snoozes",
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    }

    pub fn next_snooze(self) -> Result<Option<Millis>, StoreError> {
        self.optional(
            "SELECT MIN(snooze_until) FROM threads WHERE snooze_until IS NOT NULL",
            [],
            "find next snooze",
            |r| r.get::<_, Option<Millis>>(0),
        )
        .map(Option::flatten)
    }

    pub fn thread_sync_states(
        self,
        account: AccountId,
    ) -> Result<Vec<ThreadSyncState>, StoreError> {
        self.all(
            "SELECT id, gmail_id, history_id FROM threads WHERE account_id = ?1",
            [account],
            "read thread sync states",
            |r| {
                Ok(ThreadSyncState {
                    id: r.get(0)?,
                    gmail_id: r.get(1)?,
                    history_id: r.get(2)?,
                })
            },
        )
    }

    pub fn owed_threads(self, account: AccountId) -> Result<Vec<GmailThreadId>, StoreError> {
        self.all(
            "SELECT gmail_id FROM threads WHERE account_id = ?1 AND history_id IS NULL ORDER BY sort_time DESC",
            [account],
            "list owed threads",
            |r| r.get(0),
        )
    }

    pub fn known_message_ids(
        self,
        account: AccountId,
        gmail_ids: &[GmailMessageId],
    ) -> Result<HashSet<GmailMessageId>, StoreError> {
        if gmail_ids.is_empty() {
            return Ok(HashSet::new());
        }
        let encoded = id_list(gmail_ids)?;
        let rows = self.all(
            "SELECT gmail_id FROM messages WHERE account_id = ?1 AND gmail_id IN (SELECT value FROM json_each(?2))",
            params![account, encoded],
            "check known message ids",
            |r| r.get::<_, GmailMessageId>(0),
        )?;
        Ok(rows.into_iter().collect())
    }

    pub fn threads_needing_bodies(self, ids: &[ThreadId]) -> Result<HashSet<ThreadId>, StoreError> {
        if ids.is_empty() {
            return Ok(HashSet::new());
        }
        let encoded = id_list(ids)?;
        let rows = self.all(
            "SELECT DISTINCT thread_id FROM messages WHERE is_draft = 0 AND has_body = 0 AND thread_id IN (SELECT value FROM json_each(?1))",
            [encoded],
            "find threads needing bodies",
            |r| r.get::<_, ThreadId>(0),
        )?;
        Ok(rows.into_iter().collect())
    }

    fn load_message_facts(self, tid: ThreadId) -> Result<Vec<MessageFacts>, StoreError> {
        self.all(
            "SELECT id, gmail_id, internal_date, from_name, from_email, to_addrs, cc_addrs, subject, snippet, message_id_hdr, in_reply_to, references_hdr, is_list, is_draft, has_attachments, label_ids
             FROM messages WHERE thread_id = ?1 ORDER BY internal_date, id",
            [tid],
            "load message facts",
            |r| {
                let labels: Vec<LabelId> = json_col(r, 15)?;
                Ok(MessageFacts {
                    id: r.get(0)?,
                    gmail_id: r.get(1)?,
                    date: r.get(2)?,
                    from: Addr {
                        name: r.get(3)?,
                        email: r.get(4)?,
                    },
                    to: json_col(r, 5)?,
                    cc: json_col(r, 6)?,
                    bcc: Vec::new(),
                    reply_to: Vec::new(),
                    subject: r.get(7)?,
                    snippet: r.get(8)?,
                    message_id_hdr: r.get(9)?,
                    in_reply_to: r.get(10)?,
                    references: json_col(r, 11)?,
                    is_list: r.get::<_, i64>(12)? != 0,
                    is_draft: r.get::<_, i64>(13)? != 0,
                    has_attachments: r.get::<_, i64>(14)? != 0,
                    labels: labels.into_iter().collect(),
                })
            },
        )
    }

    fn thread_labels_for(self, id: ThreadId) -> Result<Vec<LabelId>, StoreError> {
        self.all(
            "SELECT label_id FROM thread_labels WHERE thread_id = ?1 ORDER BY label_id",
            [id],
            "read thread labels",
            |r| r.get(0),
        )
    }

    fn attach_thread_labels(
        self,
        rows: &mut [ThreadRow],
        ids_json: &str,
    ) -> Result<(), StoreError> {
        let pairs = self.all(
            "SELECT thread_id, label_id FROM thread_labels WHERE thread_id IN (SELECT value FROM json_each(?1)) ORDER BY label_id",
            [ids_json],
            "batch read thread labels",
            |r| Ok((r.get::<_, ThreadId>(0)?, r.get::<_, LabelId>(1)?)),
        )?;
        let mut grouped: HashMap<ThreadId, Vec<LabelId>> = HashMap::new();
        for (tid, lid) in pairs {
            grouped.entry(tid).or_default().push(lid);
        }
        for row in rows {
            if let Some(labels) = grouped.remove(&row.id) {
                row.labels = labels;
            }
        }
        Ok(())
    }

    fn attachments_for_messages(
        self,
        msgs: &[MessageRecord],
    ) -> Result<HashMap<MessageId, Vec<AttachmentRecord>>, StoreError> {
        if msgs.is_empty() {
            return Ok(HashMap::new());
        }
        let ids: Vec<MessageId> = msgs.iter().map(|m| m.id).collect();
        let encoded = id_list(&ids)?;
        let sql = format!(
            "SELECT {ATT_COLS} FROM attachments WHERE message_id IN (SELECT value FROM json_each(?1)) ORDER BY id"
        );
        let atts = self.all(
            &sql,
            [encoded],
            "batch read attachments",
            AttachmentRecord::from_row,
        )?;
        let mut grouped: HashMap<MessageId, Vec<AttachmentRecord>> = HashMap::new();
        for att in atts {
            grouped.entry(att.message).or_default().push(att);
        }
        Ok(grouped)
    }
}

impl WriteTx<'_> {
    pub fn place_thread(
        &mut self,
        account: AccountId,
        gmail_id: &GmailThreadId,
        history_id: Option<HistoryId>,
    ) -> Result<ThreadId, StoreError> {
        let rev = self.stamp();
        let id = self.read().one(
            "INSERT INTO threads (account_id, gmail_id, history_id, rev) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id, gmail_id) DO UPDATE SET history_id = COALESCE(excluded.history_id, threads.history_id)
             RETURNING id",
            params![account, gmail_id, history_id, rev],
            "place thread",
            |r| r.get::<_, ThreadId>(0),
        )?;
        Ok(id)
    }

    pub fn upsert_message(
        &mut self,
        account: AccountId,
        thread: ThreadId,
        msg: &NewMessage,
    ) -> Result<MessageId, StoreError> {
        let rev = self.stamp();
        let encoded = EncodedMessage::encode(msg, rev)?;
        let existing: Option<(MessageId, ThreadId)> = self.read().optional(
            "SELECT id, thread_id FROM messages WHERE account_id = ?1 AND gmail_id = ?2",
            params![account, msg.gmail_id],
            "lookup message",
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let msg_id = if let Some((id, old_thread)) = existing {
            self.update_message_row(id, thread, msg, &encoded)?;
            if old_thread != thread {
                self.derive_thread(old_thread)?;
            }
            id
        } else {
            self.insert_message_row(account, thread, msg, &encoded)?
        };

        self.reindex_fts(msg_id, msg, None)?;
        self.touch(Some(account), Table::Messages);
        Ok(msg_id)
    }

    fn update_message_row(
        &self,
        id: MessageId,
        thread: ThreadId,
        msg: &NewMessage,
        encoded: &EncodedMessage,
    ) -> Result<(), StoreError> {
        self.read().execute(
            "UPDATE messages SET thread_id = ?2, internal_date = ?3, from_name = ?4, from_email = ?5, to_addrs = ?6, cc_addrs = ?7, bcc_addrs = ?8, reply_to = ?9, subject = ?10, snippet = ?11, message_id_hdr = ?12, in_reply_to = ?13, references_hdr = ?14, is_list = ?15, is_draft = ?16, is_live = ?17, sm_draft_id = COALESCE(?18, sm_draft_id), label_ids = ?19, has_attachments = COALESCE(?20, has_attachments), rev = ?21 WHERE id = ?1",
            params![
                id,
                thread,
                msg.date,
                msg.from.name,
                msg.from.email,
                encoded.to,
                encoded.cc,
                encoded.bcc,
                encoded.reply_to,
                msg.subject,
                msg.snippet,
                msg.message_id_hdr,
                msg.in_reply_to,
                encoded.references,
                i64::from(msg.is_list),
                i64::from(msg.is_draft),
                i64::from(encoded.is_live),
                msg.sm_draft_id,
                encoded.labels,
                msg.has_attachments.map(i64::from),
                encoded.rev,
            ],
            "update message",
        )?;
        Ok(())
    }

    fn insert_message_row(
        &self,
        account: AccountId,
        thread: ThreadId,
        msg: &NewMessage,
        encoded: &EncodedMessage,
    ) -> Result<MessageId, StoreError> {
        let id = self.read().insert(
            "INSERT INTO messages (account_id, thread_id, gmail_id, internal_date, from_name, from_email, to_addrs, cc_addrs, bcc_addrs, reply_to, subject, snippet, message_id_hdr, in_reply_to, references_hdr, is_list, is_draft, is_live, sm_draft_id, label_ids, has_attachments, rev)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
            params![
                account,
                thread,
                msg.gmail_id,
                msg.date,
                msg.from.name,
                msg.from.email,
                encoded.to,
                encoded.cc,
                encoded.bcc,
                encoded.reply_to,
                msg.subject,
                msg.snippet,
                msg.message_id_hdr,
                msg.in_reply_to,
                encoded.references,
                i64::from(msg.is_list),
                i64::from(msg.is_draft),
                i64::from(encoded.is_live),
                msg.sm_draft_id,
                encoded.labels,
                i64::from(msg.has_attachments.unwrap_or(false)),
                encoded.rev,
            ],
            "insert message",
        )?;
        Ok(MessageId(id))
    }

    pub fn store_message_body(
        &mut self,
        msg_id: MessageId,
        body: &MessageBodyWrite<'_>,
    ) -> Result<(), StoreError> {
        let Some(record) = self.read().message(msg_id)? else {
            return Ok(());
        };
        let rev = self.stamp();
        let has_att = body.attachments.iter().any(|a| !a.is_inline);
        self.read().execute(
            "UPDATE messages SET has_body = 1, body_html = ?2, quote_html = ?3, body_text = ?4, has_attachments = ?5, rev = ?6 WHERE id = ?1",
            params![msg_id, body.html, body.quote_html, body.body_text, i64::from(has_att), rev],
            "store message body",
        )?;
        self.replace_attachments(msg_id, body.attachments)?;
        let from_text = format_addr_fts(&record.from);
        let to_text = format_addrs_fts(&record.to);
        let cc_text = format_addrs_fts(&record.cc);
        let db = self.read();
        db.execute(
            "DELETE FROM messages_fts WHERE rowid = ?1",
            [msg_id],
            "delete old fts",
        )?;
        db.execute(
            "INSERT INTO messages_fts (rowid, from_text, to_text, cc_text, subject, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![msg_id, from_text, to_text, cc_text, record.subject, body.body_text],
            "insert body fts",
        )?;
        self.touch(Some(record.account), Table::Messages);
        self.touch(Some(record.account), Table::Attachments);
        self.derive_thread(record.thread)?;
        Ok(())
    }

    fn replace_attachments(
        &self,
        msg_id: MessageId,
        attachments: &[NewAttachment],
    ) -> Result<(), StoreError> {
        let existing: HashMap<String, Option<String>> = self
            .read()
            .attachments_for_message(msg_id)?
            .into_iter()
            .map(|a| (a.part_id, a.local_path))
            .collect();
        let keep: HashSet<&str> = attachments.iter().map(|a| a.part_id.as_str()).collect();
        for old_part in existing.keys() {
            if !keep.contains(old_part.as_str()) {
                self.read().execute(
                    "DELETE FROM attachments WHERE message_id = ?1 AND part_id = ?2",
                    params![msg_id, old_part],
                    "delete removed attachment",
                )?;
            }
        }
        for att in attachments {
            let local_path = att
                .local_path
                .clone()
                .or_else(|| existing.get(&att.part_id).and_then(Clone::clone));
            self.read().execute(
                "INSERT INTO attachments (message_id, part_id, filename, mime_type, size, gmail_att_id, content_id, is_inline, local_path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(message_id, part_id) DO UPDATE SET filename = excluded.filename, mime_type = excluded.mime_type, size = excluded.size, gmail_att_id = excluded.gmail_att_id, content_id = excluded.content_id, is_inline = excluded.is_inline, local_path = COALESCE(excluded.local_path, attachments.local_path)",
                params![
                    msg_id,
                    att.part_id,
                    att.filename,
                    att.mime_type,
                    att.size,
                    att.gmail_att_id,
                    att.content_id,
                    i64::from(att.is_inline),
                    local_path,
                ],
                "upsert attachment",
            )?;
        }
        Ok(())
    }

    pub fn set_attachment_path(
        &mut self,
        attachment: AttachmentId,
        path: &str,
    ) -> Result<(), StoreError> {
        let Some((account, _, rec)) = self.read().attachment(attachment)? else {
            return Ok(());
        };
        self.read().execute(
            "UPDATE attachments SET local_path = ?2 WHERE id = ?1",
            params![attachment, path],
            "set attachment local path",
        )?;
        self.touch(Some(account), Table::Attachments);
        if let Some(msg) = self.read().message(rec.message)? {
            self.derive_thread(msg.thread)?;
        }
        Ok(())
    }

    pub fn set_message_labels(
        &mut self,
        msg_id: MessageId,
        labels: &LabelSet,
    ) -> Result<Option<ThreadId>, StoreError> {
        let Some(msg) = self.read().message(msg_id)? else {
            return Ok(None);
        };
        if msg.labels == *labels {
            return Ok(Some(msg.thread));
        }
        let rev = self.stamp();
        let is_live =
            !msg.is_draft && !labels.has(SystemLabel::Trash) && !labels.has(SystemLabel::Spam);
        let encoded = to_json(&labels.to_vec())?;
        self.read().execute(
            "UPDATE messages SET label_ids = ?2, is_live = ?3, rev = ?4 WHERE id = ?1",
            params![msg_id, encoded, i64::from(is_live), rev],
            "set message labels",
        )?;
        self.touch(Some(msg.account), Table::Messages);
        Ok(Some(msg.thread))
    }

    pub fn delete_message(&mut self, msg_id: MessageId) -> Result<Option<ThreadId>, StoreError> {
        let Some(msg) = self.read().message(msg_id)? else {
            return Ok(None);
        };
        let rev = self.stamp();
        let target_drafts: Vec<DraftId> = self.read().all(
            "SELECT id FROM drafts WHERE target_message_id = ?1",
            [msg_id],
            "find drafts targeting deleted message",
            |r| r.get(0),
        )?;
        self.read().execute(
            "DELETE FROM messages_fts WHERE rowid = ?1",
            [msg_id],
            "delete message fts",
        )?;
        self.read().execute(
            "DELETE FROM messages WHERE id = ?1",
            [msg_id],
            "delete message",
        )?;
        for did in target_drafts {
            self.read().execute(
                "UPDATE drafts SET rev = ?2 WHERE id = ?1",
                params![did, rev],
                "stamp unlinked draft",
            )?;
            self.record().draft(msg.account, did);
        }
        self.touch(Some(msg.account), Table::Messages);
        self.derive_thread(msg.thread)?;
        Ok(Some(msg.thread))
    }

    pub fn delete_thread(&mut self, id: ThreadId) -> Result<(), StoreError> {
        let Some(row) = self.read().thread_row(id)? else {
            return Ok(());
        };
        let rev = self.stamp();
        let drafts: Vec<DraftId> = self.read().all(
            "SELECT id FROM drafts WHERE thread_id = ?1",
            [id],
            "find thread drafts",
            |r| r.get(0),
        )?;
        self.read().execute(
            "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE thread_id = ?1)",
            [id],
            "delete thread fts",
        )?;
        self.read()
            .execute("DELETE FROM threads WHERE id = ?1", [id], "delete thread")?;
        for did in drafts {
            self.read().execute(
                "UPDATE drafts SET rev = ?2 WHERE id = ?1",
                params![did, rev],
                "stamp draft after thread delete",
            )?;
            self.record().draft(row.account, did);
        }
        self.record().thread(row.account, id);
        self.touch(Some(row.account), Table::Messages);
        Ok(())
    }

    pub fn set_thread_snooze(
        &mut self,
        id: ThreadId,
        snooze_until: Option<Millis>,
    ) -> Result<(), StoreError> {
        let Some(row) = self.read().thread_row(id)? else {
            return Ok(());
        };
        if row.snooze_until == snooze_until {
            return Ok(());
        }
        let rev = self.stamp();
        self.read().execute(
            "UPDATE threads SET snooze_until = ?2, rev = ?3 WHERE id = ?1",
            params![id, snooze_until, rev],
            "update thread snooze",
        )?;
        self.record().thread(row.account, id);
        Ok(())
    }

    pub fn set_thread_history_id(
        &mut self,
        id: ThreadId,
        history_id: HistoryId,
    ) -> Result<(), StoreError> {
        let Some((account, _)) = self.read().thread_gmail_id(id)? else {
            return Ok(());
        };
        let changed = self.read().execute(
            "UPDATE threads SET history_id = ?2 WHERE id = ?1 AND (history_id IS NULL OR history_id != ?2)",
            params![id, history_id],
            "set thread history_id",
        )?;
        if changed > 0 {
            self.touch(Some(account), Table::SyncCursor);
        }
        Ok(())
    }

    pub fn clear_thread_history_ids(
        &mut self,
        account: AccountId,
        ids: &[ThreadId],
    ) -> Result<(), StoreError> {
        if ids.is_empty() {
            return Ok(());
        }
        let encoded = id_list(ids)?;
        let changed = self.read().execute(
            "UPDATE threads SET history_id = NULL WHERE account_id = ?1 AND id IN (SELECT value FROM json_each(?2))",
            params![account, encoded],
            "clear thread history ids",
        )?;
        if changed > 0 {
            self.touch(Some(account), Table::SyncCursor);
        }
        Ok(())
    }

    pub fn derive_thread(&mut self, id: ThreadId) -> Result<(), StoreError> {
        let Some(row) = self.read().thread_row(id)? else {
            return Ok(());
        };
        let facts = self.read().load_message_facts(id)?;
        if facts.is_empty() {
            return self.delete_thread(id);
        }
        let thread = Thread::new(facts);
        let Some(digest) = thread.digest() else {
            return self.delete_thread(id);
        };
        let split_id = if digest.in_inbox() {
            self.route_thread(row.account, &thread)?
        } else {
            None
        };
        let rev = self.stamp();
        self.read().execute(
            "UPDATE threads SET subject = ?2, snippet = ?3, sender = ?4, sender_email = ?5, sort_time = ?6, message_count = ?7, unread = ?8, has_attachments = ?9, has_live = ?10, in_inbox = ?11, split_id = ?12, rev = ?13 WHERE id = ?1",
            params![
                id,
                digest.subject,
                digest.snippet,
                digest.sender,
                digest.sender_email,
                digest.sort_time,
                i64::from(digest.message_count),
                i64::from(digest.unread),
                i64::from(digest.has_attachments),
                i64::from(digest.has_live),
                i64::from(digest.in_inbox()),
                split_id,
                rev,
            ],
            "update derived thread",
        )?;
        self.read().execute(
            "DELETE FROM thread_labels WHERE thread_id = ?1",
            [id],
            "clear thread_labels",
        )?;
        for label in &digest.labels {
            self.read().execute(
                "INSERT INTO thread_labels (thread_id, account_id, label_id) VALUES (?1, ?2, ?3)",
                params![id, row.account, label],
                "insert thread label",
            )?;
        }
        self.record().thread(row.account, id);
        Ok(())
    }

    pub fn reroute_inbox(&mut self, account: AccountId) -> Result<(), StoreError> {
        let inbox: Vec<(ThreadId, Option<SplitId>)> = self.read().all(
            "SELECT id, split_id FROM threads WHERE account_id = ?1 AND in_inbox = 1",
            [account],
            "list inbox threads for reroute",
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        for (tid, old_split) in inbox {
            let facts = self.read().load_message_facts(tid)?;
            let thread = Thread::new(facts);
            let new_split = self.route_thread(account, &thread)?;
            if new_split != old_split {
                let rev = self.stamp();
                self.read().execute(
                    "UPDATE threads SET split_id = ?2, rev = ?3 WHERE id = ?1",
                    params![tid, new_split, rev],
                    "update thread split",
                )?;
                self.record().thread(account, tid);
            }
        }
        Ok(())
    }

    fn route_thread(
        &mut self,
        account: AccountId,
        thread: &Thread,
    ) -> Result<Option<SplitId>, StoreError> {
        if !self.routing.contains_key(&account) {
            let db = self.read();
            let Some(acct) = db.account(account)? else {
                return Ok(None);
            };
            let splits = db.splits(account)?;
            let pins = db.pins(account)?;
            let user_labels = db.user_label_names(account)?;
            self.routing.insert(
                account,
                CachedRouting {
                    router: Router::new(splits, pins),
                    identity: acct.identity(),
                    user_labels,
                },
            );
        }
        Ok(self
            .routing
            .get(&account)
            .and_then(|c| c.router.route(thread, &c.identity, &c.user_labels)))
    }

    fn reindex_fts(
        &self,
        msg_id: MessageId,
        msg: &NewMessage,
        body_override: Option<&str>,
    ) -> Result<(), StoreError> {
        let db = self.read();
        let existing_body: Option<String> = match body_override {
            Some(b) => Some(b.to_owned()),
            None => db
                .optional(
                    "SELECT body_text FROM messages WHERE id = ?1",
                    [msg_id],
                    "read existing body_text",
                    |r| r.get::<_, Option<String>>(0),
                )?
                .flatten(),
        };
        let body = existing_body.as_deref().unwrap_or(&msg.snippet);
        let from_text = format_addr_fts(&msg.from);
        let to_text = format_addrs_fts(&msg.to);
        let cc_text = format_addrs_fts(&msg.cc);
        db.execute(
            "DELETE FROM messages_fts WHERE rowid = ?1",
            [msg_id],
            "delete old fts",
        )?;
        db.execute(
            "INSERT INTO messages_fts (rowid, from_text, to_text, cc_text, subject, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![msg_id, from_text, to_text, cc_text, msg.subject, body],
            "insert fts row",
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MessageBodyWrite<'a> {
    pub html: &'a str,
    pub quote_html: Option<&'a str>,
    pub body_text: &'a str,
    pub attachments: &'a [NewAttachment],
}

struct EncodedMessage {
    is_live: bool,
    rev: Rev,
    to: String,
    cc: String,
    bcc: String,
    reply_to: String,
    references: String,
    labels: String,
}

impl EncodedMessage {
    fn encode(msg: &NewMessage, rev: Rev) -> Result<Self, StoreError> {
        Ok(Self {
            is_live: !msg.is_draft
                && !msg.labels.has(SystemLabel::Trash)
                && !msg.labels.has(SystemLabel::Spam),
            rev,
            to: to_json(&msg.to)?,
            cc: to_json(&msg.cc)?,
            bcc: to_json(&msg.bcc)?,
            reply_to: to_json(&msg.reply_to)?,
            references: to_json(&msg.references)?,
            labels: to_json(&msg.labels.to_vec())?,
        })
    }
}

fn format_addr_fts(addr: &Addr) -> String {
    match addr.name.as_deref().filter(|n| !n.is_empty()) {
        Some(name) => format!("{name} {}", addr.email),
        None => addr.email.clone(),
    }
}

fn format_addrs_fts(addrs: &[Addr]) -> String {
    addrs
        .iter()
        .map(format_addr_fts)
        .collect::<Vec<_>>()
        .join(" ")
}
