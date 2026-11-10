//! Accounts and the app: the rail, an account's labels, splits, pins, outbox,
//! undo history, and notices, and the global settings.

use serde::{Deserialize, Serialize};
use sm_domain::{
    AccountId, DraftId, LabelId, NoticeId, NoticeKind, OutboxId, PinKind, SplitId, SplitKind,
    SyncStatus, ThreadId, UndoEntryId, UndoKind,
};
use ts_rs::TS;

use crate::OutboxRow;

/// The app subscription's data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppState {
    /// Every account, in rail order.
    pub accounts: Vec<AccountInfo>,
    /// The settings that apply to every account.
    pub settings: GlobalSettings,
    /// Gmail can be reached: false from a call that finds no connection
    /// until the next answer from Gmail.
    pub online: bool,
}

/// Settings that apply to every account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GlobalSettings {
    /// Messages load their remote images. On until the user turns it off.
    pub remote_images: bool,
}

/// One account, as the rail shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountInfo {
    /// Its id. An account removed and added again gets a new one.
    pub id: AccountId,
    /// The address it signed in with.
    pub email: String,
    /// The display name, from the primary `sendAs` entry; empty until it is
    /// known.
    pub name: String,
    /// The avatar's color as a hex value, such as `#714CB6`.
    pub hue: String,
    /// Its place on the rail, lowest first.
    pub position: i32,
    /// Unread Inbox threads that are not snoozed, for the avatar's count.
    pub inbox_unread: u32,
    /// Google refused its refresh token, or none is stored: the account
    /// shows Sign in again.
    pub needs_signin: bool,
    /// Its notifications are off, and its unread mail is out of the Dock
    /// badge.
    pub muted: bool,
    /// The sync state the rail shows. Sign in again overrides the rest.
    pub sync_status: SyncStatus,
    /// Gmail's `sendAs` addresses: the From picker's choices, and "this
    /// account's addresses" wherever the spec says so.
    pub send_as: Vec<SendAs>,
}

/// One of an account's Gmail send-as addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SendAs {
    /// The address.
    pub email: String,
    /// Its display name; empty for none.
    pub name: String,
    /// Gmail's default for new mail: a new message or a forward sends from
    /// it.
    pub is_default: bool,
    /// The account's own address, whose name is the account's display name.
    pub is_primary: bool,
}

/// An account subscription's data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountSummary {
    /// The account, as the app state lists it. An account that does not
    /// exist yet has its id and nothing else.
    pub account: AccountInfo,
    /// Its labels: system ones first, then user ones by name.
    pub labels: Vec<LabelInfo>,
    /// Its Split Inbox splits in split order: custom ones, then Important,
    /// then Other.
    pub splits: Vec<SplitInfo>,
    /// Its pins, of every split, turned off or not.
    pub pins: Vec<PinInfo>,
    /// Its sends still in the outbox, oldest first: what the send toasts
    /// show.
    pub outbox: Vec<OutboxRow>,
    /// The undo history. Newest last.
    pub undo: Vec<UndoEntryInfo>,
    /// Notices the UI has not acked yet, oldest first.
    pub notices: Vec<Notice>,
    /// How many actions wait in its queue for Gmail. Remove account says how
    /// many would be lost.
    pub queued_ops: u32,
}

/// One label, system or user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LabelInfo {
    /// Its local id: Gmail's id for a label Gmail made, the UI's for one
    /// made here.
    pub id: LabelId,
    /// Its name. A nested label's is the whole path, as Gmail gives it.
    pub name: String,
    /// A Gmail system label such as `INBOX`, not one the user made.
    pub system: bool,
}

/// One Split Inbox split. Unsaved ones from the splits sheet have ids below 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SplitInfo {
    /// Its id; below 1 until the sheet's save gives it one.
    pub id: SplitId,
    /// The name in the list header and on its section.
    pub name: String,
    /// A custom split, or the built-in Important or Other.
    pub kind: SplitKind,
    /// A custom split's rule, in the split query language; empty for
    /// Important and Other.
    pub query: String,
    /// Turned on. A split turned off keeps its rule and pins and is skipped.
    /// Other is always on.
    pub enabled: bool,
    /// Its name and section are hidden while it holds no conversation.
    pub hide_when_empty: bool,
    /// Its place in split order, lowest first.
    pub position: i32,
}

/// A pin that routes a sender, domain, or label to a split.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PinInfo {
    /// What `value` names: a sender, a domain, or a label.
    pub kind: PinKind,
    /// The sender's address, the domain, or the label's name, lowercased.
    pub value: String,
    /// The split its conversations go to.
    pub split: SplitId,
}

/// One undo history entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UndoEntryInfo {
    /// The number of the action that made it: what an `undo` action names.
    pub id: UndoEntryId,
    /// What it undoes: a mail action, a send, or a pin.
    pub kind: UndoKind,
    /// What the action is called, such as `Marked done`.
    pub label: String,
    /// The threads a mail entry changed; empty for a send or a pin.
    #[ts(type = "number[]")]
    pub threads: Vec<ThreadId>,
    /// A send entry's outbox row; `None` for a mail action or a pin.
    pub outbox: Option<OutboxId>,
}

/// A notice waiting for the UI to show and ack it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Notice {
    /// Its id: what the `ackNotice` action names.
    pub id: NoticeId,
    /// Why it waits, which decides how the UI shows it.
    pub kind: NoticeKind,
    /// What its toast says.
    pub message: String,
    /// The draft it is about: the one `Open draft` reopens after a send that
    /// was not sent. `None` when it is about no draft.
    pub draft: Option<DraftId>,
    /// The thread that draft replies on, so its reply composer reopens
    /// there. `None` for a new message, and for every other notice.
    pub thread: Option<ThreadId>,
}
