//! What a message is, as written and as read back.

use rusqlite::Row;
use ub_domain::{
    AccountId, Addr, AttachmentId, DraftId, GmailMessageId, LabelSet, MessageFacts, MessageId,
    Millis, Rev, ThreadId,
};

use crate::db::json_column;

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

    pub(super) fn into_facts(self) -> MessageFacts {
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

pub(super) fn message_row(r: &Row<'_>) -> rusqlite::Result<MessageRecord> {
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

pub(super) fn attachment_row(r: &Row<'_>) -> rusqlite::Result<AttachmentRecord> {
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
