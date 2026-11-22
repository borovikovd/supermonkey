use serde::{Deserialize, Serialize};

use crate::LabelId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum View {
    Inbox,
    Done,
    AllMail,
    Snoozed,
    Starred,
    Sent,
    Drafts,
    Trash,
    Spam,
    Label { label: LabelId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum DraftKind {
    New,
    Reply,
    Forward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum OutboxState {
    Waiting,
    Sending,
    Sent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum SplitKind {
    Custom,
    Important,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum PinKind {
    Sender,
    Domain,
    Label,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum SyncStatus {
    Idle,
    Syncing,
    Offline,
    Retrying,
    NeedsSignin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    Rejected,
    SendFailed,
    SendUndone,
    DraftGone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ModifyKind {
    Done,
    ReturnToInbox,
    Snooze,
    Trash,
    Spam,
    Star,
    Unstar,
    Read,
    Unread,
    Label,
    RemoveLabel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingKey {
    RemoteImages,
    Muted,
}

impl SettingKey {
    #[must_use]
    pub const fn default_on(self) -> bool {
        match self {
            Self::RemoteImages => true,
            Self::Muted => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpKind {
    Modify,
    LabelCreate,
    DraftUpsert,
    DraftDelete,
    TrashMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpQueue {
    Mail,
    Drafts,
}

impl OpQueue {
    pub const ALL: [Self; 2] = [Self::Mail, Self::Drafts];
}

impl OpKind {
    #[must_use]
    pub const fn queue(self) -> OpQueue {
        match self {
            Self::Modify | Self::LabelCreate | Self::TrashMessage => OpQueue::Mail,
            Self::DraftUpsert | Self::DraftDelete => OpQueue::Drafts,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftWait {
    Parts,
    TargetBody,
}
