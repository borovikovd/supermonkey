//! Subscriptions and the channel: what the UI subscribes to, and the updates
//! each writer commit pushes back (SPEC UI state).

use serde::{Deserialize, Serialize};
use sm_domain::{AccountId, ActionNo, DraftId, Rev, SplitId, SubId, ThreadId, View};
use ts_rs::TS;

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
        /// Opened from Trash, Spam, or a search that looks there: every
        /// non-draft message is listed.
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
        /// Search only: the query has a term local search can't evaluate,
        /// so only Gmail's hits are listed.
        needs_gmail: bool,
        /// Search only: the query looks in Trash or Spam (`in:trash`,
        /// `in:spam`, `in:anywhere`), so a conversation opened from its
        /// results lists their messages (SPEC Reading).
        looks_in_trash: bool,
        /// Gmail has another page for this list.
        more: bool,
        /// The key of the lowest row that came from Gmail's pages, if any
        /// is listed: the next page loads as the list nears it (SPEC
        /// Search), since that page may hold mail newer than the local rows
        /// below it.
        oldest_hit: Option<String>,
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
    /// Actions the core refused. Each is listed once, in or before the first
    /// message whose `ack` covers it; an action `ack` covers that no message
    /// listed was applied.
    pub rejected: Vec<Rejection>,
    pub updates: Vec<SubUpdate>,
}

/// An action the core refused (SPEC UI state, Actions).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Rejection {
    /// The refused action's number.
    pub n: ActionNo,
    /// Why, in a sentence for the user.
    pub message: String,
    /// A `saveSplits` refused for a custom split's rule: that split.
    pub split: Option<SplitId>,
}
