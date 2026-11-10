//! One thread's conversation: its messages, drafts, and pending sends.

use serde::{Deserialize, Serialize};
use sm_domain::{
    AccountId, AttachmentId, DraftId, DraftKind, GmailMessageId, LabelId, MessageId, Millis,
    OutboxId, OutboxState, Rev, ThreadId, UndoEntryId,
};
use ts_rs::TS;

use crate::Addr;

/// A conversation subscription's data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConversationData {
    /// The thread the subscription named, also when it is gone.
    pub thread: ThreadId,
    /// The account the thread is in.
    pub account: AccountId,
    /// The first non-draft message's subject that is not empty.
    pub subject: String,
    /// Label ids the thread is in, by the rule in SPEC Views.
    pub labels: Vec<LabelId>,
    /// When its snooze ends. `None` when it is not snoozed.
    pub snooze_until: Option<Millis>,
    /// Oldest first, already filtered for the view (trash mode or not), no drafts.
    pub messages: Vec<MessageSummary>,
    /// Its reply and forward drafts, oldest edit first, shown collapsed
    /// under the last message. A draft in the outbox is left out.
    pub drafts: Vec<DraftSummary>,
    /// Its sends in the outbox, shown as Sending or Sent rows. A sent one
    /// leaves once the thread holds its message.
    pub outbox: Vec<OutboxRow>,
    /// The thread no longer exists locally. The other fields are then empty.
    pub gone: bool,
    /// The `rev` of the thread's last change. A change to its messages,
    /// labels, drafts, or outbox rows stamps it too.
    pub rev: Rev,
}

/// A message's headers and flags; the body comes separately (`MessageBody`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent flags the UI reads per message."
)]
pub struct MessageSummary {
    /// Its id: what `get_bodies` takes.
    pub id: MessageId,
    /// The From mailbox.
    pub from: Addr,
    /// The To mailboxes.
    pub to: Vec<Addr>,
    /// The Cc mailboxes.
    pub cc: Vec<Addr>,
    /// The Bcc mailboxes.
    pub bcc: Vec<Addr>,
    /// The decoded Subject.
    pub subject: String,
    /// Gmail's snippet: a collapsed message's one line, and an expanded
    /// one's text until its body loads.
    pub snippet: String,
    /// Gmail's `internalDate`.
    pub date: Millis,
    /// It has `UNREAD`.
    pub unread: bool,
    /// It has `STARRED`.
    pub starred: bool,
    /// It has `TRASH`. Such a message is listed only in trash mode.
    pub in_trash: bool,
    /// It has `SPAM`. Such a message is listed only in trash mode.
    pub in_spam: bool,
    /// Its body is stored, so `get_bodies` returns it. False until a full
    /// fetch of its thread lands.
    pub has_body: bool,
    /// Plain-text mail; render with the app's own type.
    pub plain: bool,
    /// Its parts that are files, inline images included. Empty until its
    /// body is stored.
    pub attachments: Vec<AttachmentInfo>,
    /// The `rev` of its last change.
    pub rev: Rev,
}

/// A message's sanitized body, from the `get_bodies` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MessageBody {
    /// The message it is the body of.
    pub id: MessageId,
    /// The sanitized HTML above the quoted history. Plain-text mail comes
    /// as HTML too.
    pub html: String,
    /// The quoted history, sanitized, shown collapsed behind `•••`. `None`
    /// when the message quotes nothing.
    pub quote_html: Option<String>,
}

/// One attachment of a stored message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AttachmentInfo {
    /// Its id: what `open_attachment` takes.
    pub id: AttachmentId,
    /// The file's name.
    pub filename: String,
    /// The file's MIME type.
    pub mime_type: String,
    /// The size in bytes.
    // ts-rs prints `i64` as `bigint`; it stays far below 2^53.
    #[ts(type = "number")]
    pub size: i64,
    /// An inline image, which the body shows: it gets no chip.
    pub inline: bool,
    /// Its file is on this Mac, so opening it needs no download.
    pub downloaded: bool,
}

/// A collapsed draft shown under a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftSummary {
    /// The draft: clicking its line resumes it in the reply composer.
    pub id: DraftId,
    /// Reply or forward: `r` and `Enter` resume a reply draft, `f` a
    /// forward draft.
    pub kind: DraftKind,
    /// The message it is aimed at, when it names one.
    pub target: Option<MessageId>,
    /// It was started on what was then the thread's latest message, so it
    /// resumes from the latest message after new replies too.
    pub started_on_latest: bool,
    /// Its To recipients.
    pub to: Vec<Addr>,
    /// Its body as plain text.
    pub snippet: String,
    /// Its last edit: the newest draft of a kind is the one resumed.
    pub updated_at: Millis,
}

/// One send in the outbox: in its undo window, at Gmail, or sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OutboxRow {
    /// Its id: what the `closeToast` action names.
    pub id: OutboxId,
    /// The account it sends from.
    pub account: AccountId,
    /// The draft it sends: the one a composer reopens when the send is
    /// undone or not sent.
    pub draft: DraftId,
    /// The thread it replies on or, once sent, the thread its message
    /// landed on. `None` for a new message not stored yet.
    pub thread: Option<ThreadId>,
    /// When the undo window ends and the send starts.
    pub send_after: Millis,
    /// Waiting out its undo window, sending, or sent.
    pub state: OutboxState,
    /// Gmail's id of the sent message. `None` until Gmail accepts the send,
    /// and for one settled as sent whose message was not found.
    pub sent_message_gmail_id: Option<GmailMessageId>,
    /// An undo is held on it until the send resolves: the toast reads
    /// `Undoing…`.
    pub undo_requested: bool,
    /// Its toast was closed, so none shows for it again.
    pub toast_closed: bool,
    /// It missed its time: the toast and the thread read `Email delayed`
    /// until Gmail accepts it.
    pub delayed: bool,
    /// The draft's To, for the pending message shown in the thread. Empty
    /// when the draft is gone.
    pub to: Vec<Addr>,
    /// The draft's body as plain text, for that pending message.
    pub snippet: String,
    /// The draft's subject, which the send toast shows.
    pub subject: String,
    /// The undo entry for this send, if it is still in the history.
    pub undo_entry: Option<UndoEntryId>,
}
