use rusqlite::Row;
use sm_domain::{
    AccountId, Addr, AttachmentId, DraftId, GmailMessageId, LabelSet, MessageFacts, MessageId,
    Millis, Rev, ThreadId,
};

use crate::db::json_column;

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub message_id_header: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: String,
    pub is_list: bool,
    pub is_draft: bool,
    pub sm_draft: Option<DraftId>,
    pub labels: LabelSet,
    pub has_attachments: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewBody {
    pub html: Option<String>,
    pub quote_html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<NewAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAttachment {
    pub part_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: i64,
    pub gmail_attachment_id: Option<String>,
    pub content_id: Option<String>,
    pub is_inline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent stored flags of one message"
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
    pub message_id_header: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: String,
    pub is_list: bool,
    pub is_draft: bool,
    pub sm_draft: Option<DraftId>,
    pub labels: LabelSet,
    pub has_attachments: bool,
    pub has_body: bool,
    pub rev: Rev,
}

impl MessageRecord {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyRecord {
    pub id: MessageId,
    pub html: Option<String>,
    pub quote_html: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRecord {
    pub id: AttachmentId,
    pub message: MessageId,
    pub part_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: i64,
    pub gmail_attachment_id: Option<String>,
    pub content_id: Option<String>,
    pub is_inline: bool,
    pub local_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentSource {
    pub attachment: AttachmentRecord,
    pub account: AccountId,
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
        sm_draft: r.get("sm_draft_id")?,
        labels: json_column(r, "label_ids")?,
        has_attachments: r.get("has_attachments")?,
        has_body: r.get("has_body")?,
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
