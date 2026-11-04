//! Thread and draft lists: their rows and the patches that reorder them.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use ub_domain::{DraftId, LabelId, Millis, Rev, SplitId, ThreadId};

/// One row of a list. Threads use their id as `key`; the Drafts list uses draft ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListRow {
    pub key: String,
    pub thread: Option<ThreadId>,
    pub draft: Option<DraftId>,
    pub sender: String,
    pub sender_email: String,
    pub subject: String,
    pub snippet: String,
    // The latest message's date, by the rule in SPEC Views.
    pub time: Millis,
    pub unread: bool,
    pub starred: bool,
    pub has_attachments: bool,
    pub message_count: u32,
    /// Label ids the thread is in, by the rule in SPEC Views (system and user).
    pub labels: Vec<LabelId>,
    pub snooze_until: Option<Millis>,
    // The Split Inbox split the row is in; none outside the inbox.
    pub split: Option<SplitId>,
    pub rev: Rev,
}

// One key of a list's order, with the split it is in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListEntry {
    pub key: String,
    pub split: Option<SplitId>,
}

/// Insert or move `key` so it follows `after` (None = first).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListMove {
    pub key: String,
    pub split: Option<SplitId>,
    pub after: Option<String>,
}
