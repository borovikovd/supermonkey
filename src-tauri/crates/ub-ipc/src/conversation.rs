//! One thread's conversation: its messages, drafts, and pending sends.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use ub_domain::{
    AccountId, AttachmentId, DraftId, DraftKind, GmailMessageId, LabelId, MessageId, Millis,
    OutboxId, OutboxState, Rev, ThreadId, UndoEntryId,
};

use crate::Addr;

// A conversation subscription's data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConversationData {
    pub thread: ThreadId,
    pub account: AccountId,
    pub subject: String,
    pub labels: Vec<LabelId>,
    pub snooze_until: Option<Millis>,
    /// Oldest first, already filtered for the view (trash mode or not), no drafts.
    pub messages: Vec<MessageSummary>,
    pub drafts: Vec<DraftSummary>,
    pub outbox: Vec<OutboxRow>,
    /// The thread no longer exists locally.
    pub gone: bool,
    pub rev: Rev,
}

// A message's headers and flags; the body comes separately (`MessageBody`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent flags the UI reads per message."
)]
pub struct MessageSummary {
    pub id: MessageId,
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub snippet: String,
    pub date: Millis,
    pub unread: bool,
    pub starred: bool,
    /// Sent from one of this account's addresses.
    pub from_me: bool,
    pub in_trash: bool,
    pub in_spam: bool,
    pub has_body: bool,
    /// Plain-text mail; render with the app's own type.
    pub plain: bool,
    pub attachments: Vec<AttachmentInfo>,
    pub rev: Rev,
}

// A message's sanitized body, from the `get_bodies` command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MessageBody {
    pub id: MessageId,
    pub html: String,
    pub quote_html: Option<String>,
}

// One attachment of a stored message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AttachmentInfo {
    pub id: AttachmentId,
    pub filename: String,
    pub mime_type: String,
    // In bytes. ts-rs prints `i64` as `bigint`; it stays far below 2^53.
    #[ts(type = "number")]
    pub size: i64,
    pub inline: bool,
    pub downloaded: bool,
}

/// A collapsed draft shown under a thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftSummary {
    pub id: DraftId,
    pub kind: DraftKind,
    pub target: Option<MessageId>,
    pub started_on_latest: bool,
    pub to: Vec<Addr>,
    pub snippet: String,
    pub updated_at: Millis,
}

// One send in the outbox: in its undo window, at Gmail, or sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OutboxRow {
    pub id: OutboxId,
    pub account: AccountId,
    pub draft: DraftId,
    pub thread: Option<ThreadId>,
    pub send_after: Millis,
    pub state: OutboxState,
    pub sent_message_gmail_id: Option<GmailMessageId>,
    pub undo_requested: bool,
    pub toast_closed: bool,
    pub delayed: bool,
    /// For the pending message shown in the thread.
    pub to: Vec<Addr>,
    pub snippet: String,
    pub subject: String,
    /// The undo entry for this send, if it is still in the history.
    pub undo_entry: Option<UndoEntryId>,
}
