//! Work for the async side that a commit asks for.

use ub_domain::{AccountId, DraftId, LabelId, SubId, ThreadId};
use ub_gmail::PageToken;

/// Work for the async side after a commit. Jobs record effects on the
/// writer state; the writer releases them to the router only once the
/// commit has landed, and drops them when it rolls back.
///
/// Each later slice adds the variants its workers consume (the op queue,
/// the outbox, notifications, the snooze timer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// A conversation was subscribed: fetch its bodies if any are missing
    /// (SPEC Sync, priority 2).
    FetchThread {
        /// The thread's account.
        account: AccountId,
        /// The thread.
        thread: ThreadId,
    },
    /// Run a Gmail search or view listing for a list subscription (SPEC
    /// Search, Views).
    GmailList {
        /// The subscription its hits go to.
        sub: SubId,
        /// The account to list.
        account: AccountId,
        /// What to list.
        query: GmailQuery,
        /// The page to fetch; `None` for the first.
        page: Option<PageToken>,
    },
    /// A subscription closed: cancel its queued Gmail work.
    CancelSub(SubId),
    /// A draft read from Gmail opened in a composer and needs its parts
    /// downloaded. Filled by the compose slice.
    DraftParts {
        /// The draft's account.
        account: AccountId,
        /// The draft.
        draft: DraftId,
    },
    /// A draft's row is gone: delete its attachment files. Filled by the
    /// compose slice.
    DraftFilesGone(DraftId),
    /// The UI is showing this account. Its Gmail requests need no head
    /// start: each account has its own slots and budget (DEVIATIONS).
    OnScreen(AccountId),
    /// A message named a label the app does not know: read the labels again
    /// (SPEC Sync).
    RefreshLabels(AccountId),
    /// An account was removed and its rows are gone: stop its worker, revoke
    /// and delete its refresh token, and delete its files (SPEC Accounts).
    AccountRemoved {
        /// The account.
        account: AccountId,
        /// Its address, which keys its refresh token.
        email: String,
    },
    /// Unread counts or mute settings may have changed: redraw the Dock badge.
    Badge,
}

impl Effect {
    /// The variant's name, for logs.
    pub fn name(&self) -> &'static str {
        match self {
            Self::FetchThread { .. } => "fetch thread",
            Self::GmailList { .. } => "gmail list",
            Self::CancelSub(_) => "cancel sub",
            Self::DraftParts { .. } => "draft parts",
            Self::DraftFilesGone(_) => "draft files gone",
            Self::OnScreen(_) => "on screen",
            Self::RefreshLabels(_) => "refresh labels",
            Self::AccountRemoved { .. } => "account removed",
            Self::Badge => "badge",
        }
    }
}

/// The Gmail `threads.list` a list subscription merges into its local rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmailQuery {
    /// A search box query, as Gmail should get it; it waits for typing to
    /// pause.
    Search(String),
    /// A view's `q=` query: Done's and All Mail's.
    View(String),
    /// Threads with a label.
    Label {
        /// Gmail's id for the label.
        gmail_label: LabelId,
        /// Set `includeSpamTrash=true` (Trash and Spam).
        include_spam_trash: bool,
    },
}
