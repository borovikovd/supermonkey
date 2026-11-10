//! UI actions: what the UI asks the core to do, numbered so the channel can
//! ack them.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sm_domain::{
    AccountId, ActionNo, DraftId, LabelId, Millis, ModifyKind, NoticeId, OutboxId, PinKind,
    SplitId, SubId, ThreadId, UndoEntryId,
};
use ts_rs::TS;

use crate::{DraftSave, SplitInfo, SubKey};

/// One action with its number; numbers only grow (`LaunchInfo::last_action`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NumberedAction {
    /// Its number, one more than the action before it. The core drops a
    /// number it has already received.
    pub n: ActionNo,
    /// What the user did.
    pub action: Action,
}

/// A [`NumberedAction`] as `submit` reads it: the number strictly, the
/// action on its own, so one the core can't read is refused under its
/// number while the rest of its batch applies (SPEC UI state, Actions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submitted {
    /// The action's number. A batch with an action that has none is not
    /// read at all.
    pub n: ActionNo,
    /// The action, or an [`Unreadable`] when the core couldn't read it.
    pub action: Result<Action, Unreadable>,
}

/// An action the core couldn't read. Only its `type` is kept: the rest may
/// hold the user's text, which never goes to the logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    /// The action's `type`, if it named one.
    pub kind: Option<String>,
}

impl<'de> Deserialize<'de> for Submitted {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// The envelope: a page that can't number its actions sent nothing usable.
        #[derive(Deserialize)]
        struct Envelope {
            n: ActionNo,
            action: Value,
        }
        let Envelope { n, action } = Envelope::deserialize(deserializer)?;
        let kind = action
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned);
        #[expect(
            clippy::map_err_ignore,
            reason = "serde's message can quote the user's text, which never reaches the logs"
        )]
        let action = Action::deserialize(action).map_err(|_| Unreadable { kind });
        Ok(Self { n, action })
    }
}

impl From<NumberedAction> for Submitted {
    fn from(NumberedAction { n, action }: NumberedAction) -> Self {
        Self {
            n,
            action: Ok(action),
        }
    }
}

/// Something the user did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Action {
    /// First call after a page load: closes every subscription of the previous page.
    Reset,
    /// Open a subscription.
    Subscribe {
        /// The id the page made for it. Its updates and its `unsubscribe`
        /// name it.
        sub: SubId,
        /// What it watches.
        key: SubKey,
    },
    /// Close a subscription.
    Unsubscribe {
        /// The subscription to close.
        sub: SubId,
    },
    /// Change threads' labels or snooze.
    Modify(ModifyAction),
    /// Undo a history entry.
    Undo {
        /// The account whose undo history holds the entry.
        account: AccountId,
        /// The entry to undo. The action is refused if it is gone.
        entry: UndoEntryId,
    },
    /// Create a label (with a UI-made local id) and optionally apply it in the same action.
    CreateLabel {
        /// The account the label is made in.
        account: AccountId,
        /// The id the page made for it, kept as its local id.
        id: LabelId,
        /// The label's name, as the user typed it.
        name: String,
        /// The label change to make with it. `None` only creates the label.
        apply: Option<ModifyAction>,
    },
    /// Create or update a draft.
    SaveDraft {
        /// The draft's id, made by the page when its composer opened.
        draft: DraftId,
        /// The draft's creation, or its fields after an edit.
        save: DraftSave,
    },
    /// The user closed a composer; its draft stays.
    CloseComposer {
        /// The draft whose composer closed. An untouched fresh one is
        /// discarded.
        draft: DraftId,
    },
    /// Queue a draft in the outbox.
    SendDraft {
        /// The draft to send.
        draft: DraftId,
    },
    /// Delete a draft.
    DiscardDraft {
        /// The draft to delete, here and on Gmail.
        draft: DraftId,
    },
    /// The user closed a send toast.
    CloseToast {
        /// The send whose toast closed.
        outbox: OutboxId,
    },
    /// The UI showed a notice.
    AckNotice {
        /// The notice shown, which the core then drops.
        notice: NoticeId,
    },
    /// Change a setting (SPEC Settings).
    SetSetting(Setting),
    /// Pin a sender, domain, or label to a split.
    Pin {
        /// The account whose Inbox the pin routes.
        account: AccountId,
        /// What `value` names: a sender, a domain, or a label.
        kind: PinKind,
        /// The sender's address, the domain, or the label's name.
        value: String,
        /// The split to pin it to.
        split: SplitId,
    },
    /// Save the splits sheet.
    SaveSplits {
        /// The account whose splits the sheet edited.
        account: AccountId,
        /// The splits to keep, in their new order; `position` is not read.
        /// One with an id below 1 is a new custom split.
        splits: Vec<SplitInfo>,
        /// Ids of custom splits to delete.
        #[ts(type = "number[]")]
        deleted: Vec<SplitId>,
    },
    /// Sign out and forget an account.
    RemoveAccount {
        /// The account to remove.
        account: AccountId,
    },
    /// Fetch older search pages / view pages from Gmail.
    MoreResults {
        /// The search or view list that neared its oldest Gmail hit.
        sub: SubId,
    },
}

/// A setting the user changes, with its new value (SPEC Settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "setting", rename_all = "camelCase")]
#[ts(export)]
pub enum Setting {
    /// Load remote images in messages, in every account.
    RemoteImages {
        /// Whether they load.
        on: bool,
    },
    /// Mute an account: no notifications, and its unread mail leaves the Dock badge.
    Muted {
        /// The account to mute or unmute.
        account: AccountId,
        /// Whether it is muted.
        on: bool,
    },
}

/// A resolved mail change, never a toggle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModifyAction {
    /// The account the threads are in, fixed when the user acted.
    pub account: AccountId,
    /// What the user did: it names the undo entry, and done, trash, and spam
    /// clear a snooze.
    pub kind: ModifyKind,
    /// The threads to change. One that is gone by now is skipped.
    #[ts(type = "number[]")]
    pub threads: Vec<ThreadId>,
    /// Label ids to add to the threads' messages.
    pub add: Vec<LabelId>,
    /// Label ids to remove from the threads' messages.
    pub remove: Vec<LabelId>,
    /// The snooze time to set or clear. `None` leaves it to `kind`.
    pub snooze: Option<SnoozeChange>,
    /// Read-on-open is not an undo entry.
    pub undoable: bool,
}

/// A change to threads' snooze time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "at", rename_all = "camelCase")]
#[ts(export)]
pub enum SnoozeChange {
    /// Snooze until this time.
    Set(Millis),
    /// Wake now.
    Clear,
}
