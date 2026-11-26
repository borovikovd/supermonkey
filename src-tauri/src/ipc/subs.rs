use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::domain::{AccountId, ActionNo, DraftId, Rev, SplitId, SubId, ThreadId, View};

use super::{AccountSummary, AppState, ConversationData, DraftState, ListEntry, ListMove, ListRow};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum SubKey {
    App,
    Account {
        account: AccountId,
    },
    List {
        account: AccountId,
        view: View,
    },
    Search {
        account: AccountId,
        query: String,
    },
    Conversation {
        thread: ThreadId,
        #[serde(rename = "trashMode")]
        trash_mode: bool,
        open: bool,
    },
    Composer {
        draft: DraftId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum SubUpdate {
    #[serde(rename_all = "camelCase")]
    List {
        sub: SubId,
        first: bool,
        ids: Option<Vec<ListEntry>>,
        removed: Vec<String>,
        moved: Vec<ListMove>,
        rows: Vec<ListRow>,
        searching: bool,
        needs_gmail: bool,
        looks_in_trash: bool,
        terms: Vec<String>,
        more: bool,
        oldest_hit: Option<String>,
    },
    Conversation {
        sub: SubId,
        first: bool,
        data: ConversationData,
    },
    Composer {
        sub: SubId,
        first: bool,
        draft: DraftState,
    },
    Account {
        sub: SubId,
        first: bool,
        summary: AccountSummary,
    },
    App {
        sub: SubId,
        first: bool,
        state: AppState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ChannelMessage {
    pub rev: Rev,
    pub ack: ActionNo,
    pub rejected: Vec<Rejection>,
    pub updates: Vec<SubUpdate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Rejection {
    pub n: ActionNo,
    pub message: String,
    pub reason: RejectReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum RejectReason {
    SplitRule { split: SplitId },
    Other,
}
