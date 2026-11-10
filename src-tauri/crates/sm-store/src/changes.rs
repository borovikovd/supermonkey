//! What a commit changed, recorded by the write methods themselves.

use std::collections::BTreeSet;

use sm_domain::{AccountId, DraftId, MessageId, OutboxId, Rev, ThreadId};

/// A kind of data an account owns in the store, as [`Changes`] records a
/// write to it. Each is one table, except that Gmail's sync bookkeeping is
/// kept apart from the rows it sits beside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Table {
    /// The account row: its address, name, `sendAs` list, sync state, and
    /// sign-in state.
    Account,
    /// Gmail sync bookkeeping: the account's history cursor, when it was
    /// read, and whether the connect sync finished; each thread's history id.
    SyncCursor,
    /// Labels.
    Labels,
    /// Thread rows: derived from their messages (every message write
    /// re-derives its thread), snoozed, routed to a split, or stamped
    /// because their drafts or outbox rows changed.
    Threads,
    /// Drafts.
    Drafts,
    /// The outbox.
    Outbox,
    /// The Gmail op queue.
    Ops,
    /// The undo history.
    Undo,
    /// Notices.
    Notices,
    /// Splits.
    Splits,
    /// Pins of senders, domains, and labels to splits.
    Pins,
    /// Settings; [`AccountId::NONE`] owns the global ones.
    Settings,
    /// Contacts for autocomplete.
    Contacts,
}

impl Table {
    /// Every table, as removing an account writes them all.
    pub const ALL: [Self; 13] = [
        Self::Account,
        Self::SyncCursor,
        Self::Labels,
        Self::Threads,
        Self::Drafts,
        Self::Outbox,
        Self::Ops,
        Self::Undo,
        Self::Notices,
        Self::Splits,
        Self::Pins,
        Self::Settings,
        Self::Contacts,
    ];
}

/// The rows and tables one write transaction changed. Every write method
/// records its own entries, so callers never mark anything by hand. It says
/// only what was written: which read models that makes stale is core's
/// business.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    /// Thread rows written, stamped, or deleted.
    pub threads: BTreeSet<ThreadId>,
    /// Message rows written or deleted, bodies included. Their threads are
    /// re-derived, so they are in `threads` too.
    pub messages: BTreeSet<MessageId>,
    /// Draft rows written or deleted, and drafts whose outbox row came or
    /// went.
    pub drafts: BTreeSet<DraftId>,
    /// Outbox rows written or deleted.
    pub outbox: BTreeSet<OutboxId>,
    /// Each account's tables that had a row added, changed, or removed.
    pub tables: BTreeSet<(AccountId, Table)>,
}

impl Changes {
    /// Whether nothing was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether `account`'s `table` was written.
    #[must_use]
    pub fn touched(&self, account: AccountId, table: Table) -> bool {
        self.tables.contains(&(account, table))
    }

    /// Whether any account's `table` was written.
    #[must_use]
    pub fn touched_any(&self, table: Table) -> bool {
        self.tables.iter().any(|&(_, t)| t == table)
    }

    pub(crate) fn touch(&mut self, account: AccountId, table: Table) {
        self.tables.insert((account, table));
    }

    pub(crate) fn thread(&mut self, account: AccountId, thread: ThreadId) {
        self.threads.insert(thread);
        self.touch(account, Table::Threads);
    }

    pub(crate) fn message(&mut self, message: MessageId) {
        self.messages.insert(message);
    }

    pub(crate) fn draft(&mut self, account: AccountId, draft: DraftId) {
        self.drafts.insert(draft);
        self.touch(account, Table::Drafts);
    }

    pub(crate) fn outbox(&mut self, account: AccountId, outbox: OutboxId) {
        self.outbox.insert(outbox);
        self.touch(account, Table::Outbox);
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
