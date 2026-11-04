//! Messages: headers, labels, bodies, attachments, and their search index.
//! Every write re-derives the threads it touches (see `routing`).

use std::collections::BTreeSet;

use rusqlite::{Row, named_params};
use ub_domain::{
    AccountId, Addr, AttachmentId, DraftId, GmailMessageId, GmailThreadId, LabelSet, MessageChange,
    MessageFacts, MessageId, Millis, Rev, SystemLabel, ThreadId,
};

use crate::StoreError;
use crate::db::{Db, id_list, json_column, to_json};
use crate::routing;
use crate::tx::{WriteTx, require};

/// A message as a Gmail fetch gives it, parsed: everything but the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessage {
    /// Gmail's id.
    pub gmail_id: GmailMessageId,
    /// Gmail's `internalDate`.
    pub date: Millis,
    /// The `From` mailbox.
    pub from: Addr,
    /// The `To` mailboxes.
    pub to: Vec<Addr>,
    /// The `Cc` mailboxes.
    pub cc: Vec<Addr>,
    /// The `Bcc` mailboxes.
    pub bcc: Vec<Addr>,
    /// The `Reply-To` mailboxes.
    pub reply_to: Vec<Addr>,
    /// The decoded `Subject`.
    pub subject: String,
    /// Gmail's snippet.
    pub snippet: String,
    /// The `Message-ID` header.
    pub message_id_header: Option<String>,
    /// The `In-Reply-To` header.
    pub in_reply_to: Option<String>,
    /// The `References` header, as is.
    pub references: String,
    /// Has `List-Id` or `List-Unsubscribe`.
    pub is_list: bool,
    /// A Gmail draft's message.
    pub is_draft: bool,
    /// The `X-Ubermensch-Draft` header: the local draft this app wrote it as.
    pub ub_draft: Option<DraftId>,
    /// Its own labels, as local label ids.
    pub labels: LabelSet,
    /// Has an attachment that is not inline.
    pub has_attachments: bool,
}

/// A message's body as a full fetch gives it, sanitized.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewBody {
    /// The sanitized HTML; `None` for plain-text mail.
    pub html: Option<String>,
    /// The quoted history, sanitized, shown collapsed.
    pub quote_html: Option<String>,
    /// The plain text, which search indexes.
    pub text: Option<String>,
    /// Plain-text mail, shown in the app's own type.
    pub is_plain: bool,
    /// Its parts that are files, inline ones included.
    pub attachments: Vec<NewAttachment>,
}

/// One part of a message that is a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAttachment {
    /// Gmail's `partId`, unique within the message.
    pub part_id: String,
    /// The file name.
    pub filename: String,
    /// The MIME type.
    pub mime_type: String,
    /// The size in bytes.
    pub size: i64,
    /// Gmail's attachment id, to download it.
    pub gmail_attachment_id: Option<String>,
    /// The `Content-ID` of an inline part.
    pub content_id: Option<String>,
    /// Shown inline rather than as a file.
    pub is_inline: bool,
}

/// A stored message's headers and flags.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent stored flags of one message"
)]
pub struct MessageRecord {
    /// Its row.
    pub id: MessageId,
    /// Its account.
    pub account: AccountId,
    /// Its thread.
    pub thread: ThreadId,
    /// Gmail's id.
    pub gmail_id: GmailMessageId,
    /// Gmail's `internalDate`.
    pub date: Millis,
    /// The `From` mailbox.
    pub from: Addr,
    /// The `To` mailboxes.
    pub to: Vec<Addr>,
    /// The `Cc` mailboxes.
    pub cc: Vec<Addr>,
    /// The `Bcc` mailboxes.
    pub bcc: Vec<Addr>,
    /// The `Reply-To` mailboxes.
    pub reply_to: Vec<Addr>,
    /// The decoded `Subject`.
    pub subject: String,
    /// Gmail's snippet.
    pub snippet: String,
    /// The `Message-ID` header.
    pub message_id_header: Option<String>,
    /// The `In-Reply-To` header.
    pub in_reply_to: Option<String>,
    /// The `References` header.
    pub references: String,
    /// List mail.
    pub is_list: bool,
    /// A Gmail draft's message.
    pub is_draft: bool,
    /// The local draft this app wrote it as.
    pub ub_draft: Option<DraftId>,
    /// Its own labels.
    pub labels: LabelSet,
    /// Has an attachment that is not inline.
    pub has_attachments: bool,
    /// Its body is stored.
    pub has_body: bool,
    /// Plain-text mail.
    pub is_plain: bool,
    /// The `rev` of its last change.
    pub rev: Rev,
}

impl MessageRecord {
    /// The message as the domain rules read it.
    pub fn facts(&self) -> MessageFacts {
        MessageFacts {
            id: self.id,
            gmail_id: self.gmail_id.clone(),
            date: self.date,
            from: self.from.clone(),
            to: self.to.clone(),
            cc: self.cc.clone(),
            bcc: self.bcc.clone(),
            subject: self.subject.clone(),
            snippet: self.snippet.clone(),
            is_draft: self.is_draft,
            is_list: self.is_list,
            has_attachments: self.has_attachments,
            labels: self.labels.clone(),
        }
    }

    fn into_facts(self) -> MessageFacts {
        MessageFacts {
            id: self.id,
            gmail_id: self.gmail_id,
            date: self.date,
            from: self.from,
            to: self.to,
            cc: self.cc,
            bcc: self.bcc,
            subject: self.subject,
            snippet: self.snippet,
            is_draft: self.is_draft,
            is_list: self.is_list,
            has_attachments: self.has_attachments,
            labels: self.labels,
        }
    }
}

/// A stored body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyRecord {
    /// The message.
    pub id: MessageId,
    /// The sanitized HTML; `None` for plain-text mail.
    pub html: Option<String>,
    /// The quoted history.
    pub quote_html: Option<String>,
    /// The plain text.
    pub text: Option<String>,
    /// Plain-text mail.
    pub is_plain: bool,
}

/// A stored attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRecord {
    /// Its id.
    pub id: AttachmentId,
    /// The message it belongs to.
    pub message: MessageId,
    /// Gmail's `partId`.
    pub part_id: String,
    /// The file name.
    pub filename: String,
    /// The MIME type.
    pub mime_type: String,
    /// The size in bytes.
    pub size: i64,
    /// Gmail's attachment id.
    pub gmail_attachment_id: Option<String>,
    /// The `Content-ID` of an inline part.
    pub content_id: Option<String>,
    /// Shown inline.
    pub is_inline: bool,
    /// The downloaded file.
    pub local_path: Option<String>,
}

/// Where an attachment's bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentSource {
    /// The attachment.
    pub attachment: AttachmentRecord,
    /// Its message's account.
    pub account: AccountId,
    /// Its message's Gmail id.
    pub message_gmail_id: GmailMessageId,
}

fn message_row(r: &Row<'_>) -> rusqlite::Result<MessageRecord> {
    Ok(MessageRecord {
        id: r.get("id")?,
        account: r.get("account_id")?,
        thread: r.get("thread_id")?,
        gmail_id: r.get("gmail_id")?,
        date: r.get("internal_date")?,
        from: Addr {
            name: r.get("from_name")?,
            email: r.get("from_email")?,
        },
        to: json_column(r, "to_addrs")?,
        cc: json_column(r, "cc_addrs")?,
        bcc: json_column(r, "bcc_addrs")?,
        reply_to: json_column(r, "reply_to")?,
        subject: r.get("subject")?,
        snippet: r.get("snippet")?,
        message_id_header: r.get("message_id_hdr")?,
        in_reply_to: r.get("in_reply_to")?,
        references: r.get("references_hdr")?,
        is_list: r.get("is_list")?,
        is_draft: r.get("is_draft")?,
        ub_draft: r.get("ub_draft_id")?,
        labels: json_column(r, "label_ids")?,
        has_attachments: r.get("has_attachments")?,
        has_body: r.get("has_body")?,
        is_plain: r.get("is_plain")?,
        rev: r.get("rev")?,
    })
}

fn attachment_row(r: &Row<'_>) -> rusqlite::Result<AttachmentRecord> {
    Ok(AttachmentRecord {
        id: r.get("id")?,
        message: r.get("message_id")?,
        part_id: r.get("part_id")?,
        filename: r.get("filename")?,
        mime_type: r.get("mime_type")?,
        size: r.get("size")?,
        gmail_attachment_id: r.get("gmail_att_id")?,
        content_id: r.get("content_id")?,
        is_inline: r.get("is_inline")?,
        local_path: r.get("local_path")?,
    })
}

/// Reads of messages.
#[derive(Clone, Copy)]
pub struct MessageReads<'c> {
    db: Db<'c>,
}

impl<'c> MessageReads<'c> {
    pub(crate) fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// One message.
    pub fn get(self, id: MessageId) -> Result<Option<MessageRecord>, StoreError> {
        self.db.optional(
            "read a message",
            "SELECT id, account_id, thread_id, gmail_id, internal_date, from_name, from_email,
                    to_addrs, cc_addrs, bcc_addrs, reply_to, subject, snippet, message_id_hdr,
                    in_reply_to, references_hdr, is_list, is_draft, ub_draft_id, label_ids,
                    has_attachments, has_body, is_plain, rev
             FROM messages WHERE id = ?1",
            [id],
            message_row,
        )
    }

    /// The message Gmail knows as `gmail_id`.
    pub fn by_gmail_id(
        self,
        account: AccountId,
        gmail_id: &GmailMessageId,
    ) -> Result<Option<MessageRecord>, StoreError> {
        self.db.optional(
            "find a message by Gmail id",
            "SELECT id, account_id, thread_id, gmail_id, internal_date, from_name, from_email,
                    to_addrs, cc_addrs, bcc_addrs, reply_to, subject, snippet, message_id_hdr,
                    in_reply_to, references_hdr, is_list, is_draft, ub_draft_id, label_ids,
                    has_attachments, has_body, is_plain, rev
             FROM messages WHERE account_id = ?1 AND gmail_id = ?2",
            (account, gmail_id),
            message_row,
        )
    }

    /// A thread's messages, drafts included, oldest first (by date, then id).
    pub fn for_thread(self, thread: ThreadId) -> Result<Vec<MessageRecord>, StoreError> {
        self.db.all(
            "read a thread's messages",
            "SELECT id, account_id, thread_id, gmail_id, internal_date, from_name, from_email,
                    to_addrs, cc_addrs, bcc_addrs, reply_to, subject, snippet, message_id_hdr,
                    in_reply_to, references_hdr, is_list, is_draft, ub_draft_id, label_ids,
                    has_attachments, has_body, is_plain, rev
             FROM messages WHERE thread_id = ?1 ORDER BY internal_date, id",
            [thread],
            message_row,
        )
    }

    /// A thread's messages as the domain rules read them, oldest first.
    /// `Thread::new` over them gives the thread.
    pub fn facts(self, thread: ThreadId) -> Result<Vec<MessageFacts>, StoreError> {
        Ok(self
            .for_thread(thread)?
            .into_iter()
            .map(MessageRecord::into_facts)
            .collect())
    }

    /// The thread's non-draft message whose `Message-ID` is `header`, for a
    /// draft's `In-Reply-To`.
    pub fn by_header_id(
        self,
        thread: ThreadId,
        header: &str,
    ) -> Result<Option<MessageId>, StoreError> {
        self.db.optional(
            "find a message by Message-ID",
            "SELECT id FROM messages
             WHERE thread_id = ?1 AND message_id_hdr = ?2 AND is_draft = 0
             ORDER BY internal_date DESC, id DESC LIMIT 1",
            (thread, header),
            |r| r.get("id"),
        )
    }

    /// The Gmail draft messages that carry this local draft's id.
    pub fn draft_copies(
        self,
        account: AccountId,
        draft: &DraftId,
    ) -> Result<Vec<MessageId>, StoreError> {
        self.db.all(
            "find a draft's Gmail copies",
            "SELECT id FROM messages WHERE account_id = ?1 AND ub_draft_id = ?2 AND is_draft = 1",
            (account, draft),
            |r| r.get("id"),
        )
    }

    /// Stored bodies for these messages, in the order asked; messages with
    /// no stored body are left out.
    pub fn bodies(self, ids: &[MessageId]) -> Result<Vec<BodyRecord>, StoreError> {
        self.db.all(
            "read bodies",
            "SELECT m.id, m.body_html, m.quote_html, m.body_text, m.is_plain
             FROM json_each(?1) j JOIN messages m ON m.id = j.value
             WHERE m.has_body = 1 ORDER BY j.key",
            [id_list(ids)?],
            |r| {
                Ok(BodyRecord {
                    id: r.get("id")?,
                    html: r.get("body_html")?,
                    quote_html: r.get("quote_html")?,
                    text: r.get("body_text")?,
                    is_plain: r.get("is_plain")?,
                })
            },
        )
    }

    /// A message's plain text, empty when no body is stored.
    pub fn body_text(self, id: MessageId) -> Result<String, StoreError> {
        Ok(self
            .db
            .optional(
                "read a message's text",
                "SELECT body_text FROM messages WHERE id = ?1",
                [id],
                |r| r.get::<_, Option<String>>("body_text"),
            )?
            .flatten()
            .unwrap_or_default())
    }

    /// A message's attachments, inline parts included, in part order.
    pub fn attachments(self, message: MessageId) -> Result<Vec<AttachmentRecord>, StoreError> {
        self.db.all(
            "read a message's attachments",
            "SELECT id, message_id, part_id, filename, mime_type, size, gmail_att_id, content_id,
                    is_inline, local_path
             FROM attachments WHERE message_id = ?1 ORDER BY id",
            [message],
            attachment_row,
        )
    }

    /// An attachment with what downloading it needs.
    pub fn attachment(self, id: AttachmentId) -> Result<Option<AttachmentSource>, StoreError> {
        self.db.optional(
            "read an attachment",
            "SELECT a.id, a.message_id, a.part_id, a.filename, a.mime_type, a.size,
                    a.gmail_att_id, a.content_id, a.is_inline, a.local_path,
                    m.account_id, m.gmail_id
             FROM attachments a JOIN messages m ON m.id = a.message_id WHERE a.id = ?1",
            [id],
            |r| {
                Ok(AttachmentSource {
                    attachment: attachment_row(r)?,
                    account: r.get("account_id")?,
                    message_gmail_id: r.get("gmail_id")?,
                })
            },
        )
    }

    /// The Gmail ids of a thread's messages whose bodies are stored, so a
    /// fetch skips them ("stored bodies are never fetched again").
    pub fn gmail_ids_with_body(
        self,
        account: AccountId,
        thread: &GmailThreadId,
    ) -> Result<Vec<GmailMessageId>, StoreError> {
        self.db.all(
            "read which bodies a thread has",
            "SELECT m.gmail_id FROM messages m JOIN threads t ON t.id = m.thread_id
             WHERE t.account_id = ?1 AND t.gmail_id = ?2 AND m.has_body = 1",
            (account, thread),
            |r| r.get("gmail_id"),
        )
    }

    /// Whether a thread needs a body fetch: it is not stored, or a message of
    /// it has no body.
    pub fn thread_needs_bodies(
        self,
        account: AccountId,
        thread: &GmailThreadId,
    ) -> Result<bool, StoreError> {
        self.db.one(
            "check a thread's bodies",
            "SELECT NOT EXISTS (SELECT 1 FROM threads WHERE account_id = ?1 AND gmail_id = ?2)
                 OR EXISTS (SELECT 1 FROM messages m JOIN threads t ON t.id = m.thread_id
                            WHERE t.account_id = ?1 AND t.gmail_id = ?2 AND m.has_body = 0)
                 AS needs",
            (account, thread),
            |r| r.get("needs"),
        )
    }
}

/// Writes to messages. Each re-derives, stamps, and records the threads it
/// touches, once per call.
pub struct MessageWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> MessageWrites<'t, 'c> {
    pub(crate) fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Stores a message's headers and labels in `thread`, adding it or
    /// replacing what was stored (a metadata fetch replaces labels). Its body,
    /// if stored, stays. Returns its id.
    pub fn upsert(
        &mut self,
        thread: ThreadId,
        message: &NewMessage,
    ) -> Result<MessageId, StoreError> {
        let account = require(
            self.tx.read().threads().account_of(thread)?,
            "thread",
            &thread,
        )?;
        let mut touched = BTreeSet::from([thread]);
        let (id, _) = upsert_row(self.tx, account, thread, message, &mut touched)?;
        derive_all(self.tx, touched)?;
        Ok(id)
    }

    /// Sets messages' labels, as a sync or a merge decides them.
    pub fn set_labels(&mut self, labels: &[(MessageId, LabelSet)]) -> Result<(), StoreError> {
        let mut touched = BTreeSet::new();
        for (id, set) in labels {
            touched.insert(write_labels(self.tx, *id, set)?);
        }
        derive_all(self.tx, touched)
    }

    /// Applies an action's label changes, each to the message's current
    /// labels. Messages that are gone are skipped.
    pub fn apply(&mut self, changes: &[MessageChange]) -> Result<(), StoreError> {
        let mut touched = BTreeSet::new();
        for change in changes {
            let Some(current) = self.tx.db().optional(
                "read a message's labels",
                "SELECT label_ids FROM messages WHERE id = ?1",
                [change.message],
                |r| json_column::<LabelSet>(r, "label_ids"),
            )?
            else {
                continue;
            };
            touched.insert(write_labels(
                self.tx,
                change.message,
                &change.apply(&current),
            )?);
        }
        derive_all(self.tx, touched)
    }

    /// Stores a message's body and attachments. Attachments keep their id and
    /// downloaded file by part id; parts the body no longer has go.
    pub fn store_body(&mut self, id: MessageId, body: &NewBody) -> Result<(), StoreError> {
        let rev = self.tx.stamp();
        let db = self.tx.db();
        let thread: ThreadId = require(
            db.optional(
                "find a message's thread",
                "SELECT thread_id FROM messages WHERE id = ?1",
                [id],
                |r| r.get("thread_id"),
            )?,
            "message",
            &id,
        )?;
        db.execute(
            "store a message's body",
            "UPDATE messages SET has_body = 1, body_html = :html, quote_html = :quote,
                 body_text = :text, is_plain = :plain, rev = :rev
             WHERE id = :id",
            named_params! {
                ":id": id, ":html": body.html, ":quote": body.quote_html,
                ":text": body.text, ":plain": body.is_plain, ":rev": rev,
            },
        )?;
        for part in &body.attachments {
            db.execute(
                "store an attachment",
                "INSERT INTO attachments (message_id, part_id, filename, mime_type, size,
                     gmail_att_id, content_id, is_inline)
                 VALUES (:message, :part, :filename, :mime, :size, :gmail, :cid, :inline)
                 ON CONFLICT (message_id, part_id) DO UPDATE SET
                     filename = excluded.filename, mime_type = excluded.mime_type,
                     size = excluded.size, gmail_att_id = excluded.gmail_att_id,
                     content_id = excluded.content_id, is_inline = excluded.is_inline",
                named_params! {
                    ":message": id, ":part": part.part_id, ":filename": part.filename,
                    ":mime": part.mime_type, ":size": part.size,
                    ":gmail": part.gmail_attachment_id, ":cid": part.content_id,
                    ":inline": part.is_inline,
                },
            )?;
        }
        let parts: Vec<&str> = body
            .attachments
            .iter()
            .map(|a| a.part_id.as_str())
            .collect();
        db.execute(
            "drop attachments a body no longer has",
            "DELETE FROM attachments WHERE message_id = ?1
               AND part_id NOT IN (SELECT value FROM json_each(?2))",
            (id, id_list(&parts)?),
        )?;
        reindex(db, id)?;
        self.tx.record().message(id);
        derive_all(self.tx, BTreeSet::from([thread]))
    }

    /// Deletes messages, from Gmail's history or a refetch. Drafts aimed at
    /// one lose their target. Unknown ids are skipped.
    pub fn delete(&mut self, ids: &[MessageId]) -> Result<(), StoreError> {
        let mut touched = BTreeSet::new();
        for &id in ids {
            if let Some(thread) = delete_row(self.tx, id)? {
                touched.insert(thread);
            }
        }
        derive_all(self.tx, touched)
    }

    /// Records where an attachment was downloaded.
    pub fn set_attachment_path(&mut self, id: AttachmentId, path: &str) -> Result<(), StoreError> {
        let rev = self.tx.stamp();
        let db = self.tx.db();
        let message: MessageId = require(
            db.optional(
                "find an attachment's message",
                "SELECT message_id FROM attachments WHERE id = ?1",
                [id],
                |r| r.get("message_id"),
            )?,
            "attachment",
            &id,
        )?;
        db.execute(
            "store an attachment's file",
            "UPDATE attachments SET local_path = ?2 WHERE id = ?1",
            (id, path),
        )?;
        db.execute(
            "stamp a message",
            "UPDATE messages SET rev = ?2 WHERE id = ?1",
            (message, rev),
        )?;
        self.tx.record().message(message);
        let thread = self.tx.read().messages().get(message)?.map(|m| m.thread);
        derive_all(self.tx, thread.into_iter().collect())
    }
}

/// Inserts or updates one message row and its index entry. Adds to `touched`
/// the thread it is in and, when it moved, the one it left. Returns its id
/// and whether it is new.
pub(crate) fn upsert_row(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    thread: ThreadId,
    m: &NewMessage,
    touched: &mut BTreeSet<ThreadId>,
) -> Result<(MessageId, bool), StoreError> {
    let rev = tx.stamp();
    let db = tx.db();
    let is_live =
        !m.is_draft && !m.labels.has(SystemLabel::Trash) && !m.labels.has(SystemLabel::Spam);
    let existing: Option<(MessageId, ThreadId)> = db.optional(
        "find a stored message",
        "SELECT id, thread_id FROM messages WHERE account_id = ?1 AND gmail_id = ?2",
        (account, &m.gmail_id),
        |r| Ok((r.get("id")?, r.get("thread_id")?)),
    )?;
    let to = to_json("to_addrs", &m.to)?;
    let cc = to_json("cc_addrs", &m.cc)?;
    let bcc = to_json("bcc_addrs", &m.bcc)?;
    let reply_to = to_json("reply_to", &m.reply_to)?;
    let labels = to_json("label_ids", &m.labels)?;
    let params = named_params! {
        ":account": account, ":thread": thread, ":gmail": m.gmail_id, ":date": m.date,
        ":from_name": m.from.name, ":from_email": m.from.email, ":to": to, ":cc": cc,
        ":bcc": bcc, ":reply_to": reply_to, ":subject": m.subject, ":snippet": m.snippet,
        ":msgid": m.message_id_header, ":in_reply_to": m.in_reply_to,
        ":references": m.references, ":is_list": m.is_list, ":is_draft": m.is_draft,
        ":is_live": is_live, ":ub_draft": m.ub_draft, ":labels": labels,
        ":attachments": m.has_attachments, ":rev": rev,
    };
    let (id, is_new) = if let Some((id, old_thread)) = existing {
        db.execute(
            "update a stored message",
            "UPDATE messages SET thread_id = :thread, internal_date = :date,
                 from_name = :from_name, from_email = :from_email, to_addrs = :to,
                 cc_addrs = :cc, bcc_addrs = :bcc, reply_to = :reply_to, subject = :subject,
                 snippet = :snippet, message_id_hdr = :msgid, in_reply_to = :in_reply_to,
                 references_hdr = :references, is_list = :is_list, is_draft = :is_draft,
                 is_live = :is_live, ub_draft_id = :ub_draft, label_ids = :labels,
                 has_attachments = :attachments, rev = :rev
             WHERE account_id = :account AND gmail_id = :gmail",
            params,
        )?;
        touched.insert(old_thread);
        (id, false)
    } else {
        let id = db.insert(
            "add a message",
            "INSERT INTO messages (account_id, thread_id, gmail_id, internal_date, from_name,
                 from_email, to_addrs, cc_addrs, bcc_addrs, reply_to, subject, snippet,
                 message_id_hdr, in_reply_to, references_hdr, is_list, is_draft, is_live,
                 ub_draft_id, label_ids, has_attachments, rev)
             VALUES (:account, :thread, :gmail, :date, :from_name, :from_email, :to, :cc,
                 :bcc, :reply_to, :subject, :snippet, :msgid, :in_reply_to, :references,
                 :is_list, :is_draft, :is_live, :ub_draft, :labels, :attachments, :rev)",
            params,
        )?;
        (MessageId(id), true)
    };
    reindex(db, id)?;
    touched.insert(thread);
    tx.record().message(id);
    Ok((id, is_new))
}

/// Writes one message's labels and returns its thread.
fn write_labels(
    tx: &mut WriteTx<'_>,
    id: MessageId,
    labels: &LabelSet,
) -> Result<ThreadId, StoreError> {
    let rev = tx.stamp();
    let db = tx.db();
    let thread: ThreadId = require(
        db.optional(
            "find a message's thread",
            "SELECT thread_id FROM messages WHERE id = ?1",
            [id],
            |r| r.get("thread_id"),
        )?,
        "message",
        &id,
    )?;
    db.execute(
        "store a message's labels",
        "UPDATE messages
         SET label_ids = :labels,
             is_live = (is_draft = 0 AND NOT EXISTS (SELECT 1 FROM json_each(:labels)
                                                     WHERE value IN ('TRASH', 'SPAM'))),
             rev = :rev
         WHERE id = :id",
        named_params! { ":id": id, ":labels": to_json("label_ids", labels)?, ":rev": rev },
    )?;
    tx.record().message(id);
    Ok(thread)
}

/// Deletes one message row and its index entry; returns its thread, or
/// `None` when it was not stored.
pub(crate) fn delete_row(
    tx: &mut WriteTx<'_>,
    id: MessageId,
) -> Result<Option<ThreadId>, StoreError> {
    let rev = tx.stamp();
    let db = tx.db();
    let Some((account, thread)): Option<(AccountId, ThreadId)> = db.optional(
        "find a message's thread",
        "SELECT account_id, thread_id FROM messages WHERE id = ?1",
        [id],
        |r| Ok((r.get("account_id")?, r.get("thread_id")?)),
    )?
    else {
        return Ok(None);
    };
    let aimed: Vec<DraftId> = db.all(
        "find drafts aimed at a message",
        "SELECT id FROM drafts WHERE target_message_id = ?1",
        [id],
        |r| r.get("id"),
    )?;
    db.execute(
        "unaim drafts from a message",
        "UPDATE drafts SET target_message_id = NULL, rev = ?2 WHERE target_message_id = ?1",
        (id, rev),
    )?;
    db.execute(
        "drop a message's index entry",
        "DELETE FROM messages_fts WHERE rowid = ?1",
        [id],
    )?;
    db.execute(
        "delete a message",
        "DELETE FROM messages WHERE id = ?1",
        [id],
    )?;
    let changes = tx.record();
    changes.message(id);
    for draft in aimed {
        changes.draft(account, draft);
    }
    Ok(Some(thread))
}

/// Rebuilds a message's search index entry from its row.
fn reindex(db: Db<'_>, id: MessageId) -> Result<(), StoreError> {
    db.execute(
        "drop a message's index entry",
        "DELETE FROM messages_fts WHERE rowid = ?1",
        [id],
    )?;
    db.execute(
        "index a message",
        "INSERT INTO messages_fts (rowid, from_text, to_text, cc_text, subject, body)
         SELECT m.id,
                coalesce(m.from_name || ' ', '') || m.from_email,
                (SELECT coalesce(group_concat(coalesce(json_extract(a.value, '$.name') || ' ', '')
                                              || json_extract(a.value, '$.email'), ' '), '')
                 FROM json_each(m.to_addrs) a),
                (SELECT coalesce(group_concat(coalesce(json_extract(a.value, '$.name') || ' ', '')
                                              || json_extract(a.value, '$.email'), ' '), '')
                 FROM json_each(m.cc_addrs) a),
                m.subject,
                coalesce(m.body_text, '')
         FROM messages m WHERE m.id = ?1",
        [id],
    )?;
    Ok(())
}

/// Re-derives each thread once.
pub(crate) fn derive_all(
    tx: &mut WriteTx<'_>,
    threads: BTreeSet<ThreadId>,
) -> Result<(), StoreError> {
    for thread in threads {
        routing::derive_thread(tx, thread)?;
    }
    Ok(())
}
