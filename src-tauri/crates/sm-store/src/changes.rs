use std::collections::BTreeSet;

use sm_domain::{AccountId, DraftId, Rev, ThreadId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Table {
    Account,
    SyncCursor,
    Labels,
    Threads,
    Drafts,
    Outbox,
    Ops,
    Undo,
    Notices,
    Splits,
    Pins,
    Settings,
    Contacts,
}

impl Table {
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub threads: BTreeSet<ThreadId>,
    pub drafts: BTreeSet<DraftId>,
    pub tables: BTreeSet<(AccountId, Table)>,
}

impl Changes {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    #[must_use]
    pub fn touched(&self, account: AccountId, table: Table) -> bool {
        self.tables.contains(&(account, table))
    }

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

    pub(crate) fn draft(&mut self, account: AccountId, draft: DraftId) {
        self.drafts.insert(draft);
        self.touch(account, Table::Drafts);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub rev: Rev,
    pub changes: Changes,
}
