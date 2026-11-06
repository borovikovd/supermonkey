//! Drafts in a composer: their state, the fields a save sends, and attachments.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use ub_domain::{AccountId, AttachmentId, DraftId, DraftKind, MessageId, Rev, ThreadId};

use crate::Addr;

// A composer subscription's data: one draft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftState {
    pub id: DraftId,
    pub account: AccountId,
    pub kind: DraftKind,
    pub reply_all: bool,
    pub thread: Option<ThreadId>,
    pub target: Option<MessageId>,
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub body_html: String,
    /// Read-only quote block shown collapsed under the editor.
    pub quote_html: Option<String>,
    pub attachments: Vec<DraftAttachment>,
    // Reopened from a saved draft, as opposed to started fresh.
    pub resumed: bool,
    // Sent or deleted: the composer closes.
    pub gone: bool,
    pub rev: Rev,
}

/// The composer's full state, sent with saves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftFields {
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub body_html: String,
    pub attachments: Vec<DraftAttachment>,
}

// One save of a draft: its creation, or new fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum DraftSave {
    /// A fresh composer's first save. Rust fills From, To, Cc, Subject, and the quote.
    #[serde(rename_all = "camelCase")]
    Create {
        account: AccountId,
        kind: DraftKind,
        reply_all: bool,
        thread: Option<ThreadId>,
        target: Option<MessageId>,
        /// The composer was aimed at the thread's latest message.
        started_on_latest: bool,
    },
    /// The composer's fields after an edit.
    Update {
        fields: DraftFields,
        /// The `rev` of the draft state the fields were based on: an
        /// attachment Rust added after it stays, though the fields lack it.
        rev: Rev,
    },
}

// One attachment of a draft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftAttachment {
    /// Stable id inside the draft.
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    // In bytes. ts-rs prints `i64` as `bigint`; it stays far below 2^53.
    #[ts(type = "number")]
    pub size: i64,
    /// Local file once available.
    pub path: Option<String>,
    /// Source attachment row when carried from a forwarded message or a Gmail draft.
    pub source: Option<AttachmentId>,
}
