//! UI actions: what the UI asks the core to do, numbered so the channel can
//! ack them.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use ts_rs::TS;
use ub_domain::{
    AccountId, ActionNo, DraftId, LabelId, Millis, ModifyKind, NoticeId, OutboxId, PinKind,
    SplitId, SubId, ThreadId, UndoEntryId,
};

use crate::{DraftSave, SplitInfo, SubKey};

// One action with its number; numbers only grow (`LaunchInfo::last_action`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NumberedAction {
    pub n: ActionNo,
    pub action: Action,
}

/// A [`NumberedAction`] as `submit` reads it: the number strictly, the
/// action on its own, so one the core can't read is refused under its
/// number while the rest of its batch applies (SPEC UI state, Actions).
#[derive(Debug, Clone, PartialEq)]
pub struct Submitted {
    pub n: ActionNo,
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

// Something the user did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Action {
    /// First call after a page load: closes every subscription of the previous page.
    Reset,
    /// Open a subscription.
    Subscribe { sub: SubId, key: SubKey },
    /// Close a subscription.
    Unsubscribe { sub: SubId },
    /// Change threads' labels or snooze.
    Modify(ModifyAction),
    /// Undo a history entry.
    Undo {
        account: AccountId,
        entry: UndoEntryId,
    },
    /// Create a label (with a UI-made local id) and optionally apply it in the same action.
    CreateLabel {
        account: AccountId,
        id: LabelId,
        name: String,
        apply: Option<ModifyAction>,
    },
    /// Create or update a draft.
    SaveDraft { draft: DraftId, save: DraftSave },
    /// The user closed a composer; its draft stays.
    CloseComposer { draft: DraftId },
    /// Queue a draft in the outbox.
    SendDraft { draft: DraftId },
    /// Delete a draft.
    DiscardDraft { draft: DraftId },
    /// The user closed a send toast.
    CloseToast { outbox: OutboxId },
    /// The UI showed a notice.
    AckNotice { notice: NoticeId },
    /// Change a setting (SPEC Settings).
    SetSetting(Setting),
    /// Pin a sender, domain, or label to a split.
    Pin {
        account: AccountId,
        kind: PinKind,
        value: String,
        split: SplitId,
    },
    /// Save the splits sheet.
    SaveSplits {
        account: AccountId,
        splits: Vec<SplitInfo>,
        /// Ids of custom splits to delete.
        #[ts(type = "number[]")]
        deleted: Vec<SplitId>,
    },
    /// Sign out and forget an account.
    RemoveAccount { account: AccountId },
    /// Fetch older search pages / view pages from Gmail.
    MoreResults { sub: SubId },
}

/// A setting the user changes, with its new value (SPEC Settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "setting", rename_all = "camelCase")]
#[ts(export)]
pub enum Setting {
    /// Load remote images in messages, in every account.
    RemoteImages { on: bool },
    /// Mute an account: no notifications, and its unread mail leaves the Dock badge.
    Muted { account: AccountId, on: bool },
}

/// A resolved mail change, never a toggle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModifyAction {
    pub account: AccountId,
    pub kind: ModifyKind,
    #[ts(type = "number[]")]
    pub threads: Vec<ThreadId>,
    // Label ids to add and to remove.
    pub add: Vec<LabelId>,
    pub remove: Vec<LabelId>,
    pub snooze: Option<SnoozeChange>,
    /// Read-on-open is not an undo entry.
    pub undoable: bool,
}

// A change to threads' snooze time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "at", rename_all = "camelCase")]
#[ts(export)]
pub enum SnoozeChange {
    /// Snooze until this time.
    Set(Millis),
    /// Wake now.
    Clear,
}
