use sm_domain::{AccountId, DraftId, LabelId, SubId, ThreadId};
use sm_gmail::PageToken;

use crate::Notification;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    FetchThread {
        account: AccountId,
        thread: ThreadId,
    },
    ThreadUnneeded(ThreadId),
    GmailList {
        sub: SubId,
        account: AccountId,
        query: GmailQuery,
        page: Option<PageToken>,
    },
    CancelSub(SubId),
    DraftParts {
        account: AccountId,
        draft: DraftId,
    },
    DraftFilesGone(DraftId),
    DraftBack(DraftId),
    ReadDraft {
        account: AccountId,
        draft: DraftId,
    },
    Outbox(AccountId),
    RefreshLabels(AccountId),
    AccountRemoved {
        account: AccountId,
        email: String,
    },
    Badge,
    Ops(AccountId),
    Notify(Notification),
    Snoozes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GmailQuery {
    Search(String),
    View(String),
    Label {
        gmail_label: LabelId,
        include_spam_trash: bool,
    },
}
