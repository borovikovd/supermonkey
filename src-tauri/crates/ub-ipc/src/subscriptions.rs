//! Subscriptions and the channel: what the UI subscribes to, and the updates
//! each writer commit pushes back (SPEC UI state).

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use ub_domain::{AccountId, ActionNo, DraftId, Rev, SubId, ThreadId, View};

use crate::{AccountSummary, AppState, ConversationData, DraftState, ListEntry, ListMove, ListRow};

// What a subscription watches.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum SubKey {
    /// The account list for the rail and global settings.
    App,
    /// One per account, always open.
    Account { account: AccountId },
    /// A mailbox view's thread list.
    List { account: AccountId, view: View },
    /// A search's thread list.
    Search { account: AccountId, query: String },
    /// One thread's conversation.
    Conversation {
        thread: ThreadId,
        /// Opened from Trash or Spam: every non-draft message is listed.
        #[serde(rename = "trashMode")]
        trash_mode: bool,
        /// Open, as opposed to prefetched.
        open: bool,
    },
    /// One draft open in a composer.
    Composer { draft: DraftId },
}

// One subscription's first load or patch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum SubUpdate {
    /// `ids` is set on a first load; patches carry `removed` and `moved` instead.
    /// `rows` holds the full row of every changed id in the list (and a window on first load).
    #[serde(rename_all = "camelCase")]
    List {
        sub: SubId,
        first: bool,
        ids: Option<Vec<ListEntry>>,
        removed: Vec<String>,
        moved: Vec<ListMove>,
        rows: Vec<ListRow>,
        /// Search only: a Gmail query is still running.
        searching: bool,
        /// Gmail has another page for this list.
        more: bool,
    },
    /// The whole conversation, on a first load and on every change.
    Conversation {
        sub: SubId,
        first: bool,
        data: ConversationData,
    },
    /// The whole draft, on a first load and on every change.
    Composer {
        sub: SubId,
        first: bool,
        draft: DraftState,
    },
    /// The whole account summary, on a first load and on every change.
    Account {
        sub: SubId,
        first: bool,
        summary: AccountSummary,
    },
    /// The whole app state, on a first load and on every change.
    App {
        sub: SubId,
        first: bool,
        state: AppState,
    },
}

/// One message per writer commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ChannelMessage {
    // The commit's rev.
    pub rev: Rev,
    /// Highest UI action number applied in this snapshot.
    pub ack: ActionNo,
    pub updates: Vec<SubUpdate>,
}
