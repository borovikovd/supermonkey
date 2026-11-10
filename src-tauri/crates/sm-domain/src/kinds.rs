//! The small enums rules and storage depend on. They are also IPC types, so
//! they derive ts-rs behind the `ts` feature. Their TypeScript output is the
//! frozen contract in `src/bindings`. It carries the doc comments on the types
//! and on a variant's fields, but not the ones on the variants.

use serde::{Deserialize, Serialize};

use crate::LabelId;

/// A mailbox view the list can show (SPEC Views).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum View {
    /// Threads with `INBOX` that are not snoozed, grouped into splits.
    Inbox,
    /// Gmail's archive: threads not in Inbox and not snoozed.
    Done,
    /// Gmail's All Mail: every thread with a message that is not a draft
    /// and not in trash or spam, snoozed ones included.
    AllMail,
    /// Threads with a snooze time, whether or not it has passed.
    Snoozed,
    /// Threads with `STARRED`.
    Starred,
    /// Threads with `SENT`.
    Sent,
    /// Drafts, one row each: Gmail's, and local ones not pushed yet.
    Drafts,
    /// Threads with a message in Trash.
    Trash,
    /// Threads with a message in Spam.
    Spam,
    /// Threads with one label.
    Label {
        /// The label's local id.
        label: LabelId,
    },
}

/// What a draft answers: nothing, a reply, or a forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum DraftKind {
    /// A new message, written on the `c` screen.
    New,
    /// A reply or a reply all, from `r` or `Enter`.
    Reply,
    /// A forward, from `f`. It stays on its thread, like a reply.
    Forward,
}

/// Where an outbox send is: in its undo window, at Gmail, or done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum OutboxState {
    /// Waiting for `send_after`, or back to retry: Gmail has not got the
    /// send. An undo cancels it.
    Waiting,
    /// At Gmail: the draft is being written and sent, or the send has no
    /// clear answer yet. An undo is held until it resolves.
    Sending,
    /// Gmail accepted it. An undo trashes the sent message.
    Sent,
}

/// A split's role: a custom rule, or the built-in Important or Other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum SplitKind {
    /// A split the user made: a name and a rule.
    Custom,
    /// The built-in split for person-to-person mail. It can be turned off
    /// but not deleted.
    Important,
    /// The built-in catch-all: always last, always on.
    Other,
}

/// What a split pin matches: a sender address, a domain, or a label name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum PinKind {
    /// The sender's address. A sender pin beats a domain pin.
    Sender,
    /// The sender's domain. A domain pin beats a label pin.
    Domain,
    /// One of the thread's user labels, by name, ignoring case.
    Label,
}

/// An account's sync state, as the rail shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum SyncStatus {
    /// Gmail answered the account's last call.
    Idle,
    /// The connect sync has started and Gmail has not answered it yet.
    Syncing,
    /// A call found no connection.
    Offline,
    /// Gmail's errors have gone on for 30 s of retrying: the quiet sync
    /// status (SPEC Gmail errors).
    Retrying,
    /// Google refused the refresh token, or none is stored: the account
    /// shows Sign in again.
    NeedsSignin,
}

/// What an undo entry undoes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum UndoKind {
    /// A mail action: the labels and snooze times it changed go back.
    Mail,
    /// A send: its outbox row.
    Send,
    /// A pin: it goes, and the pin it replaced comes back.
    Pin,
}

/// Why a notice waits for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    /// Gmail rejected a queued action: an error toast.
    Rejected,
    /// Gmail rejected a send: its draft goes back in a composer.
    SendFailed,
    /// A held send undo resolved as not sent: its draft goes back in a
    /// composer.
    SendUndone,
    /// A draft open in a composer was sent or deleted elsewhere.
    DraftGone,
    /// Anything else worth a plain toast.
    Info,
}

/// What an action is called in the undo toast and history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ModifyKind {
    /// Mark done: archive and mark read. It clears a snooze.
    Done,
    /// Return to inbox, out of Done, a snooze, Trash, or Spam.
    ReturnToInbox,
    /// Snooze until a time.
    Snooze,
    /// Move to Trash. It clears a snooze.
    Trash,
    /// Mark as spam. It clears a snooze.
    Spam,
    /// Add the star.
    Star,
    /// Remove the star.
    Unstar,
    /// Mark read, on open too.
    Read,
    /// Mark unread.
    Unread,
    /// Add a label.
    Label,
    /// Remove a label.
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
    #[must_use]
    pub const fn default_on(self) -> bool {
        match self {
            Self::RemoteImages => true,
            Self::Muted => false,
        }
    }
}
