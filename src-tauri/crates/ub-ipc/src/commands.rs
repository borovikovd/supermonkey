//! Results of the commands other than `submit` (ARCHITECTURE.md, section 2).

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use ub_domain::{ActionNo, Millis};

// What `connect` tells a fresh page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaunchInfo {
    /// Continue numbering actions after this.
    pub last_action: ActionNo,
    pub debug: bool,
}

// One `autocomplete` match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContactSuggestion {
    pub name: String,
    pub email: String,
    /// Byte range in name or email that matched, for bolding.
    pub match_field: String,
    pub match_start: u32,
    pub match_end: u32,
}

// One `snooze_options` choice: its label and when it wakes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SnoozeChoice {
    pub label: String,
    pub at: Millis,
}

/// What an unsubscribe did (SPEC: Unsubscribe and block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Unsubscribed {
    /// The list's one-click URL accepted the request.
    OneClick { sender: String },
    /// An unsubscribe email went to the list.
    Emailed { sender: String },
    /// The list's unsubscribe page opened in the browser.
    OpenedPage { sender: String },
    /// The message offers no way to unsubscribe.
    NoLink { sender: String },
}

/// What Block did (SPEC: Unsubscribe and block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum BlockResult {
    /// A Gmail filter now sends mail from `from` (an address or `@domain`) to Trash.
    Blocked { from: String },
    /// The account's grant lacks Gmail filters; Sign in again adds it.
    NeedsPermission,
}

/// A sender or domain a Gmail filter sends to Trash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BlockedSender {
    /// The Gmail filter id.
    pub id: String,
    /// An address, or `@domain`.
    pub from: String,
}

/// The account's blocked senders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum BlockedSenders {
    /// The filters that block senders.
    List { senders: Vec<BlockedSender> },
    /// The account's grant lacks Gmail filters; Sign in again adds it.
    NeedsPermission,
}
