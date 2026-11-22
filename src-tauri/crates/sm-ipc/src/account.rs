use serde::{Deserialize, Serialize};
use sm_domain::{
    AccountId, DraftId, LabelId, NoticeId, NoticeKind, OutboxId, PinKind, SplitId, SplitKind,
    SyncStatus, ThreadId, UndoEntryId, UndoKind,
};
use ts_rs::TS;

use crate::OutboxRow;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppState {
    pub accounts: Vec<AccountInfo>,
    pub settings: GlobalSettings,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GlobalSettings {
    pub remote_images: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SendAs {
    pub email: String,
    pub name: String,
    pub is_default: bool,
    pub is_primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountSummary {
    pub account: AccountInfo,
    pub labels: Vec<LabelInfo>,
    pub splits: Vec<SplitInfo>,
    pub pins: Vec<PinInfo>,
    pub outbox: Vec<OutboxRow>,
    pub undo: Vec<UndoEntryInfo>,
    pub notices: Vec<Notice>,
    pub queued_ops: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LabelInfo {
    pub id: LabelId,
    pub name: String,
    pub system: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PinInfo {
    pub kind: PinKind,
    pub value: String,
    pub split: SplitId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Notice {
    pub id: NoticeId,
    pub kind: NoticeKind,
    pub message: String,
    pub draft: Option<DraftId>,
    pub thread: Option<ThreadId>,
}
