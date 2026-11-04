//! What a commit changed, recorded by the write methods themselves.

use std::collections::BTreeSet;

use ub_domain::{AccountId, DraftId, MessageId, OutboxId, Rev, ThreadId};

/// The rows and summaries one write transaction changed. Every write method
/// records its own entries, so callers never mark anything by hand; the
/// subscription registry reads this to pick the subscriptions to patch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Thread rows written or deleted: their messages, labels, snooze,
    /// split, drafts, or outbox rows changed (SPEC UI state, Ordering).
    pub threads: BTreeSet<ThreadId>,
    /// Message rows written or deleted, bodies included.
    pub messages: BTreeSet<MessageId>,
    /// Draft rows written or deleted.
    pub drafts: BTreeSet<DraftId>,
    /// Outbox rows written or deleted.
    pub outbox: BTreeSet<OutboxId>,
    /// Accounts whose thread lists may have changed: membership, order, or a
    /// thread's split.
    pub lists: BTreeSet<AccountId>,
    /// Accounts whose Drafts list may have changed.
    pub draft_lists: BTreeSet<AccountId>,
    /// Accounts whose summary changed: the account row, labels, splits,
    /// pins, outbox, undo history, notices, queued ops, settings, or the
    /// inbox unread count.
    pub summaries: BTreeSet<AccountId>,
    /// Accounts whose labels changed.
    pub labels: BTreeSet<AccountId>,
    /// Accounts whose settings changed; [`AccountId::NONE`] for the global ones.
    pub settings: BTreeSet<AccountId>,
    /// Account rows created, updated, or removed.
    pub accounts: BTreeSet<AccountId>,
    /// The app state changed: the rail, a global setting, or an unread count.
    pub app: bool,
}

impl Changes {
    /// Whether nothing was recorded.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub(crate) fn thread(&mut self, account: AccountId, thread: ThreadId) {
        self.threads.insert(thread);
        self.lists.insert(account);
        // The inbox unread count is in the summary and the rail.
        self.summaries.insert(account);
        self.app = true;
    }

    pub(crate) fn message(&mut self, message: MessageId) {
        self.messages.insert(message);
    }

    pub(crate) fn draft(&mut self, account: AccountId, draft: DraftId) {
        self.drafts.insert(draft);
        self.draft_lists.insert(account);
    }

    pub(crate) fn outbox(&mut self, account: AccountId, outbox: OutboxId) {
        self.outbox.insert(outbox);
        self.summaries.insert(account);
        // A draft with an outbox row leaves the Drafts list.
        self.draft_lists.insert(account);
    }

    pub(crate) fn summary(&mut self, account: AccountId) {
        self.summaries.insert(account);
    }

    pub(crate) fn labels(&mut self, account: AccountId) {
        self.labels.insert(account);
        self.summaries.insert(account);
    }

    pub(crate) fn setting(&mut self, account: AccountId) {
        self.settings.insert(account);
        if account == AccountId::NONE {
            self.app = true;
        } else {
            self.summaries.insert(account);
        }
    }

    pub(crate) fn account(&mut self, account: AccountId) {
        self.accounts.insert(account);
        self.summaries.insert(account);
        self.app = true;
    }
}

/// One committed write transaction, as the writer hands it to `on_commit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The `rev` of the snapshot: the one this transaction stamped when it
    /// wrote, else the previous one, unchanged.
    pub rev: Rev,
    /// What it changed; empty for a job that only read or only changed the
    /// caller's state (a subscribe, say).
    pub changes: Changes,
}
