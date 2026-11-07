//! UI actions: what the UI asks the core to do, numbered so the channel can
//! ack them.

use serde::{Deserialize, Serialize};
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
