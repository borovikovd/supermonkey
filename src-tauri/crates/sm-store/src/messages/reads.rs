//! Reads of messages, their bodies, and their attachments.

use sm_domain::{
    AccountId, AttachmentId, DraftId, GmailMessageId, GmailThreadId, MessageFacts, MessageId,
    ThreadId,
};

use super::model::{
    AttachmentRecord, AttachmentSource, BodyRecord, MessageRecord, attachment_row, message_row,
};
use crate::StoreError;
use crate::db::{Db, id_list};

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
                    in_reply_to, references_hdr, is_list, is_draft, sm_draft_id, label_ids,
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
                    in_reply_to, references_hdr, is_list, is_draft, sm_draft_id, label_ids,
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
                    in_reply_to, references_hdr, is_list, is_draft, sm_draft_id, label_ids,
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
            "SELECT id FROM messages WHERE account_id = ?1 AND sm_draft_id = ?2 AND is_draft = 1",
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
