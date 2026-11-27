use tokio::sync::mpsc;

use crate::domain::{AccountId, DraftId, GmailDraftId, GmailThreadId, SubId, ThreadId};
use crate::ipc::SubKey;

use super::runtime::{Activity, Busy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewMailAlert {
    pub(crate) thread_id: ThreadId,
    pub(crate) gmail_thread_id: GmailThreadId,
    pub(crate) sender: String,
    pub(crate) subject: String,
    pub(crate) snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    KickOps(AccountId),
    KickOutbox(AccountId),
    KickSnooze,
    RefreshBadge,
    OpenConversation {
        account: AccountId,
        thread: ThreadId,
    },
    SubscribeList {
        sub: SubId,
        account: AccountId,
        key: SubKey,
    },
    MoreResults {
        sub: SubId,
    },
    Unsubscribe(SubId),
    CancelAccountListings(AccountId),
    ResetSubs,
    FetchSubscribedBodies,
    NetworkChanged {
        online: bool,
    },
    RemoveAccount {
        account: AccountId,
        email: String,
    },
    RemoveDraftFiles(DraftId),
    RemoveAttachmentFiles(Vec<String>),
    EnsureDraftAttachments {
        account: AccountId,
        draft: DraftId,
    },
    ReloadDraftFromGmail {
        account: AccountId,
        draft: DraftId,
        gmail_draft_id: GmailDraftId,
    },
    NotifyNewMail {
        account: AccountId,
        email: String,
        items: Vec<NewMailAlert>,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct EffectBus {
    tx: mpsc::UnboundedSender<(Effect, Busy)>,
    activity: Activity,
}

impl EffectBus {
    #[must_use]
    pub(crate) fn new(activity: Activity) -> (Self, mpsc::UnboundedReceiver<(Effect, Busy)>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx, activity }, rx)
    }

    pub(crate) fn emit(&self, effect: Effect) {
        let busy = self.activity.enter();
        drop(self.tx.send((effect, busy)));
    }

    pub(crate) fn emit_all(&self, effects: impl IntoIterator<Item = Effect>) {
        for effect in effects {
            self.emit(effect);
        }
    }
}
