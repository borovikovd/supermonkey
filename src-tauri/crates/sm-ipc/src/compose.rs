//! Drafts in a composer: their state, the fields a save sends, and attachments.

use serde::{Deserialize, Serialize};
use sm_domain::{AccountId, AttachmentId, DraftId, DraftKind, MessageId, Rev, ThreadId};
use ts_rs::TS;

use crate::Addr;

/// A composer subscription's data: one draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftState {
    /// The draft's local id, made by the page.
    pub id: DraftId,
    /// The account it sends from; it never changes.
    pub account: AccountId,
    /// New, reply, or forward, kept from when the draft was started.
    pub kind: DraftKind,
    /// A reply to everyone on the target message, not only to its sender.
    pub reply_all: bool,
    /// The thread it replies on or forwards from. `None` for a new message.
    pub thread: Option<ThreadId>,
    /// The message it answers and quotes. `None` for a new message.
    pub target: Option<MessageId>,
    /// The `sendAs` address it sends from; Rust fills it at a fresh
    /// composer's first save. `None` when the draft has no From.
    pub from: Option<Addr>,
    /// The To recipients.
    pub to: Vec<Addr>,
    /// The Cc recipients.
    pub cc: Vec<Addr>,
    /// The Bcc recipients.
    pub bcc: Vec<Addr>,
    /// The subject. Rust fills a reply's and a forward's at the first save.
    pub subject: String,
    /// The editor's HTML, without the quote.
    pub body_html: String,
    /// Read-only quote block shown collapsed under the editor. `None` for a
    /// draft that quotes nothing.
    pub quote_html: Option<String>,
    /// Its files: those the user added, and those carried from a forwarded
    /// message or a Gmail draft.
    pub attachments: Vec<DraftAttachment>,
    /// Reopened from a saved draft, as opposed to started fresh: closing it
    /// never discards it as untouched.
    pub resumed: bool,
    /// Sent or deleted: the composer closes. The other fields are then
    /// empty.
    pub gone: bool,
    /// The `rev` of its last change: what an `update` save names as the
    /// state its fields were based on.
    pub rev: Rev,
}

/// The composer's full state, sent with saves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftFields {
    /// The `sendAs` address chosen in the From picker, or the one the draft
    /// loaded with.
    pub from: Option<Addr>,
    /// The To chips. Text still being typed is not among them.
    pub to: Vec<Addr>,
    /// The Cc chips.
    pub cc: Vec<Addr>,
    /// The Bcc chips.
    pub bcc: Vec<Addr>,
    /// The subject.
    pub subject: String,
    /// The editor's HTML. The quote is not in it: Rust keeps that.
    pub body_html: String,
    /// Every attachment the composer shows. One left out is removed from
    /// the draft, unless Rust added it after the `rev` the save names.
    pub attachments: Vec<DraftAttachment>,
}

/// One save of a draft: its creation, or new fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum DraftSave {
    /// A fresh composer's first save. Rust fills From, To, Cc, Subject, and the quote.
    #[serde(rename_all = "camelCase")]
    Create {
        /// The account on screen when the composer opened: the draft sends
        /// from it.
        account: AccountId,
        /// New, reply, or forward, by the key that opened the composer.
        kind: DraftKind,
        /// A reply to everyone on the target message, as `Enter` starts.
        reply_all: bool,
        /// The target message's thread. `None` for a new message.
        thread: Option<ThreadId>,
        /// The message the composer was aimed at. `None` for a new message.
        target: Option<MessageId>,
        /// The composer was aimed at the thread's latest message.
        started_on_latest: bool,
    },
    /// The composer's fields after an edit.
    Update {
        /// The composer's full state, which replaces the draft's.
        fields: DraftFields,
        /// The `rev` of the draft state the fields were based on: an
        /// attachment Rust added after it stays, though the fields lack it.
        rev: Rev,
    },
}

/// One attachment of a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftAttachment {
    /// Stable id inside the draft.
    pub id: String,
    /// The file's name, as the user picked it or as its source names it.
    pub filename: String,
    /// The file's MIME type.
    pub mime_type: String,
    /// The size in bytes.
    // ts-rs prints `i64` as `bigint`; it stays far below 2^53.
    #[ts(type = "number")]
    pub size: i64,
    /// Local file once available. `None` for a carried attachment not yet
    /// downloaded.
    pub path: Option<String>,
    /// Source attachment row when carried from a forwarded message or a Gmail draft.
    pub source: Option<AttachmentId>,
}
