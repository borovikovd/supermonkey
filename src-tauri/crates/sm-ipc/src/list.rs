//! Thread and draft lists: their rows and the patches that reorder them.

use serde::{Deserialize, Serialize};
use sm_domain::{DraftId, LabelId, Millis, Rev, SplitId, ThreadId};
use ts_rs::TS;

/// One row of a list. Threads use their id as `key`; the Drafts list uses draft ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListRow {
    /// Its key in the list: the thread id as text, or in Drafts the draft
    /// id.
    pub key: String,
    /// The thread. In Drafts, the thread the draft is on: `None` for a new
    /// message.
    pub thread: Option<ThreadId>,
    /// The draft, in Drafts. `None` in every other list.
    pub draft: Option<DraftId>,
    /// The latest message's sender, by name or else address. In Drafts, the
    /// draft's recipients, or `(no recipients)`.
    pub sender: String,
    /// The latest message's sender address. In Drafts, the first
    /// recipient's, or empty with none.
    pub sender_email: String,
    /// The first non-draft message's subject that is not empty. In Drafts,
    /// the draft's. An empty one reads `(no subject)`.
    pub subject: String,
    /// The latest message's snippet. In Drafts, the draft's body as plain
    /// text.
    pub snippet: String,
    /// The latest message's date, by the rule in SPEC Views. In Drafts, the
    /// draft's last edit.
    pub time: Millis,
    /// A message outside trash and spam is unread: the row has a dot.
    /// Always false in Drafts.
    pub unread: bool,
    /// The thread is in Starred, by the rule in SPEC Views.
    pub starred: bool,
    /// A non-draft message has an attachment, not counting inline images:
    /// the row has a paperclip. In Drafts, the draft has one.
    pub has_attachments: bool,
    /// The messages the row counts: those that are not drafts and not in
    /// trash or spam, or with none of those every non-draft. At least 1.
    pub message_count: u32,
    /// Label ids the thread is in, by the rule in SPEC Views (system and user).
    pub labels: Vec<LabelId>,
    /// When its snooze ends, shown in place of the time. `None` when the
    /// thread is not snoozed.
    pub snooze_until: Option<Millis>,
    /// The Split Inbox split the row is in. `None` outside the Inbox.
    pub split: Option<SplitId>,
    /// The `rev` of the row's last change: the mirror keeps its own copy
    /// when that one is newer.
    pub rev: Rev,
}

/// One key of a list's order, with the split it is in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListEntry {
    /// The row's key.
    pub key: String,
    /// The split the row is in, in Inbox. `None` in every other list.
    pub split: Option<SplitId>,
}

/// Insert or move `key` so it follows `after` (None = first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListMove {
    /// The key to insert or move.
    pub key: String,
    /// The split it is in now, in Inbox. `None` in every other list.
    pub split: Option<SplitId>,
    /// The key it follows in the new order. `None` puts it first.
    pub after: Option<String>,
}
