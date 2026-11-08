//! The small enums rules and storage depend on. They are also IPC types, so
//! they derive ts-rs behind the `ts` feature. Their TypeScript output is the
//! frozen contract in `src/bindings`, doc comments included, so the ones whose
//! binding has no comment are described with plain comments here.

use serde::{Deserialize, Serialize};

use crate::LabelId;

// A mailbox view the list can show (SPEC Views).
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

// What a draft answers: nothing, a reply, or a forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum DraftKind {
    New,
    Reply,
    Forward,
}

// Where an outbox send is: in its undo window, at Gmail, or done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum OutboxState {
    Waiting,
    Sending,
    Sent,
}

// A split's role: a custom rule, or the built-in Important or Other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum SplitKind {
    Custom,
    Important,
    Other,
}

// What a split pin matches: a sender address, a domain, or a label name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum PinKind {
    Sender,
    Domain,
    Label,
}

// An account's sync state, as the rail shows it.
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

// What an undo entry undoes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum UndoKind {
    Mail,
    Send,
    Pin,
}

// Why a notice waits for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    Rejected,
    SendFailed,
    SendUndone,
    DraftGone,
    Info,
}

/// What an action is called in the undo toast and history.
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

/// A setting the user turns on or off (SPEC Settings). Each is stored by
/// this key, globally or per account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingKey {
    /// Global: load remote images in messages.
    RemoteImages,
    /// Per account: no notifications, and its unread mail is out of the Dock badge.
    Muted,
}

impl SettingKey {
    /// Whether the setting is on before the user changes it.
    pub const fn default_on(self) -> bool {
        match self {
            Self::RemoteImages => true,
            Self::Muted => false,
        }
    }
}
