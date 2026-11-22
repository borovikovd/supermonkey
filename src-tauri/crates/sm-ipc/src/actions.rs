use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sm_domain::{
    AccountId, ActionNo, DraftId, LabelId, Millis, ModifyKind, NoticeId, OutboxId, PinKind,
    SplitId, SubId, ThreadId, UndoEntryId,
};
use ts_rs::TS;

use crate::{DraftSave, SplitInfo, SubKey};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NumberedAction {
    pub n: ActionNo,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submitted {
    pub n: ActionNo,
    pub action: Result<Action, Unreadable>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    pub kind: Option<String>,
}

impl<'de> Deserialize<'de> for Submitted {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Action {
    Reset,
    Subscribe {
        sub: SubId,
        key: SubKey,
    },
    Unsubscribe {
        sub: SubId,
    },
    Modify(ModifyAction),
    Undo {
        account: AccountId,
        entry: UndoEntryId,
    },
    CreateLabel {
        account: AccountId,
        id: LabelId,
        name: String,
        apply: Option<ModifyAction>,
    },
    SaveDraft {
        draft: DraftId,
        save: DraftSave,
    },
    CloseComposer {
        draft: DraftId,
    },
    SendDraft {
        draft: DraftId,
    },
    DiscardDraft {
        draft: DraftId,
    },
    CloseToast {
        outbox: OutboxId,
    },
    AckNotice {
        notice: NoticeId,
    },
    SetSetting(Setting),
    Pin {
        account: AccountId,
        kind: PinKind,
        value: String,
        split: SplitId,
    },
    SaveSplits {
        account: AccountId,
        splits: Vec<SplitInfo>,
        #[ts(type = "number[]")]
        deleted: Vec<SplitId>,
    },
    RemoveAccount {
        account: AccountId,
    },
    MoreResults {
        sub: SubId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "setting", rename_all = "camelCase")]
#[ts(export)]
pub enum Setting {
    RemoteImages { on: bool },
    Muted { account: AccountId, on: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModifyAction {
    pub account: AccountId,
    pub kind: ModifyKind,
    #[ts(type = "number[]")]
    pub threads: Vec<ThreadId>,
    pub add: Vec<LabelId>,
    pub remove: Vec<LabelId>,
    pub snooze: Option<SnoozeChange>,
    pub undoable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "at", rename_all = "camelCase")]
#[ts(export)]
pub enum SnoozeChange {
    Set(Millis),
    Clear,
}
