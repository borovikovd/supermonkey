//! Accounts and the app: the rail, an account's labels, splits, pins, outbox,
//! undo history, and notices, and the global settings.

use serde::{Deserialize, Serialize};
use sm_domain::{
    AccountId, DraftId, LabelId, NoticeId, NoticeKind, OutboxId, PinKind, SplitId, SplitKind,
    SyncStatus, ThreadId, UndoEntryId, UndoKind,
};
use ts_rs::TS;

use crate::OutboxRow;

// The app subscription's data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppState {
    pub accounts: Vec<AccountInfo>,
    pub settings: GlobalSettings,
    pub online: bool,
}

// Settings that apply to every account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GlobalSettings {
    pub remote_images: bool,
}

// One account, as the rail shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountInfo {
    pub id: AccountId,
    pub email: String,
    pub name: String,
    pub hue: String,
    pub position: i32,
    pub inbox_unread: u32,
    pub needs_signin: bool,
    pub muted: bool,
    pub sync_status: SyncStatus,
    pub send_as: Vec<SendAs>,
}

// One of an account's Gmail send-as addresses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SendAs {
    pub email: String,
    pub name: String,
    pub is_default: bool,
    pub is_primary: bool,
}

// An account subscription's data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountSummary {
    pub account: AccountInfo,
    pub labels: Vec<LabelInfo>,
    pub splits: Vec<SplitInfo>,
    pub pins: Vec<PinInfo>,
    pub outbox: Vec<OutboxRow>,
    /// Newest last.
    pub undo: Vec<UndoEntryInfo>,
    pub notices: Vec<Notice>,
    pub queued_ops: u32,
}

// One label, system or user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LabelInfo {
    pub id: LabelId,
    pub name: String,
    pub system: bool,
}

// One Split Inbox split. Unsaved ones from the splits sheet have ids below 1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SplitInfo {
    pub id: SplitId,
    pub name: String,
    pub kind: SplitKind,
    pub query: String,
    pub enabled: bool,
    pub hide_when_empty: bool,
    pub position: i32,
}

// A pin that routes a sender, domain, or label to a split.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PinInfo {
    pub kind: PinKind,
    pub value: String,
    pub split: SplitId,
}

// One undo history entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UndoEntryInfo {
    pub id: UndoEntryId,
    pub kind: UndoKind,
    pub label: String,
    #[ts(type = "number[]")]
    pub threads: Vec<ThreadId>,
    pub outbox: Option<OutboxId>,
}

// A notice waiting for the UI to show and ack it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Notice {
    pub id: NoticeId,
    pub kind: NoticeKind,
    pub message: String,
    pub draft: Option<DraftId>,
    pub thread: Option<ThreadId>,
}
