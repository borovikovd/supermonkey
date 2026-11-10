//! The write transaction and the two read views.
//!
//! [`WriteTx`] is the only way to write: it has no raw `execute`, only the
//! typed repositories. Reads are one set of methods on [`Reads`], over any
//! connection, which [`WriteTx::read`], [`Snapshot`], and [`ReadTx`] all give.

use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;

use rusqlite::Connection;
use sm_domain::{AccountId, Rev};

use crate::accounts::{AccountReads, AccountWrites};
use crate::contacts::{ContactReads, ContactWrites};
use crate::db::Db;
use crate::drafts::{DraftReads, DraftWrites};
use crate::labels::{LabelReads, LabelWrites};
use crate::messages::{MessageReads, MessageWrites};
use crate::meta::{MetaReads, MetaWrites};
use crate::notices::{NoticeReads, NoticeWrites};
use crate::ops::{OpReads, OpWrites};
use crate::outbox::{OutboxReads, OutboxWrites};
use crate::routing::Routing;
use crate::search::SearchReads;
use crate::settings::{SettingReads, SettingWrites};
use crate::splits::{SplitReads, SplitWrites};
use crate::threads::{ThreadReads, ThreadWrites};
use crate::undo::{UndoReads, UndoWrites};
use crate::{Changes, StoreError, Table};

/// One write transaction on the writer thread. Every write method stamps the
/// rows it changes with [`WriteTx::rev`] and records them in the
/// transaction's [`Changes`].
pub struct WriteTx<'c> {
    db: Db<'c>,
    rev: Rev,
    /// A row changed, so the commit moves `rev`.
    wrote: bool,
    changes: Changes,
    /// Each account's split router and identity, built on first use and
    /// dropped by any write that changes them.
    routing: HashMap<AccountId, Routing>,
    #[cfg(any(test, feature = "test-hooks"))]
    fail_commit: bool,
}

/// The transaction's `rev` and what it changed so far, not the routing it
/// caches, which holds label names.
impl fmt::Debug for WriteTx<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteTx")
            .field("rev", &self.rev)
            .field("wrote", &self.wrote)
            .field("changes", &self.changes)
            .finish_non_exhaustive()
    }
}

impl<'c> WriteTx<'c> {
    pub(crate) fn new(conn: &'c Connection, rev: Rev) -> Self {
        Self {
            db: Db::new(conn),
            rev,
            wrote: false,
            changes: Changes::default(),
            routing: HashMap::new(),
            #[cfg(any(test, feature = "test-hooks"))]
            fail_commit: false,
        }
    }

    /// The `rev` this transaction stamps on the rows it writes.
    #[must_use]
    pub const fn rev(&self) -> Rev {
        self.rev
    }

    /// What the transaction has changed so far.
    #[must_use]
    pub const fn changes(&self) -> &Changes {
        &self.changes
    }

    /// Reads inside the transaction, seeing its own writes.
    #[must_use]
    pub const fn read(&self) -> Reads<'c> {
        Reads { db: self.db }
    }

    /// The last action number and `rev`.
    pub const fn meta(&mut self) -> MetaWrites<'_, 'c> {
        MetaWrites::new(self)
    }

    /// Accounts.
    pub const fn accounts(&mut self) -> AccountWrites<'_, 'c> {
        AccountWrites::new(self)
    }

    /// Labels.
    pub const fn labels(&mut self) -> LabelWrites<'_, 'c> {
        LabelWrites::new(self)
    }

    /// Threads: storing fetched threads, snooze, deletion.
    pub const fn threads(&mut self) -> ThreadWrites<'_, 'c> {
        ThreadWrites::new(self)
    }

    /// Messages: labels, bodies, attachments.
    pub const fn messages(&mut self) -> MessageWrites<'_, 'c> {
        MessageWrites::new(self)
    }

    /// Drafts.
    pub const fn drafts(&mut self) -> DraftWrites<'_, 'c> {
        DraftWrites::new(self)
    }

    /// The outbox.
    pub const fn outbox(&mut self) -> OutboxWrites<'_, 'c> {
        OutboxWrites::new(self)
    }

    /// The Gmail op queue.
    pub const fn ops(&mut self) -> OpWrites<'_, 'c> {
        OpWrites::new(self)
    }

    /// The undo history.
    pub const fn undo(&mut self) -> UndoWrites<'_, 'c> {
        UndoWrites::new(self)
    }

    /// Notices.
    pub const fn notices(&mut self) -> NoticeWrites<'_, 'c> {
        NoticeWrites::new(self)
    }

    /// Splits and pins.
    pub const fn splits(&mut self) -> SplitWrites<'_, 'c> {
        SplitWrites::new(self)
    }

    /// Contacts for autocomplete.
    pub const fn contacts(&mut self) -> ContactWrites<'_, 'c> {
        ContactWrites::new(self)
    }

    /// Settings.
    pub const fn settings(&mut self) -> SettingWrites<'_, 'c> {
        SettingWrites::new(self)
    }

    /// Runs `step` in a savepoint, so one step of a job can fail alone: on
    /// `Err` its writes roll back, the transaction's [`Changes`] and `rev`
    /// stamping go back to what they were before it, and the transaction
    /// goes on as if `step` never ran. Core applies each UI action of a
    /// batch this way, so a rejected action never undoes the others.
    pub fn savepoint<T, E: From<StoreError>>(
        &mut self,
        step: impl FnOnce(&mut Self) -> Result<T, E>,
    ) -> Result<T, E> {
        self.db
            .execute("open a savepoint", "SAVEPOINT sm_step", [])?;
        let wrote = self.wrote;
        let changes = self.changes.clone();
        match step(self) {
            Ok(value) => {
                self.db
                    .execute("release a savepoint", "RELEASE sm_step", [])?;
                Ok(value)
            }
            Err(e) => {
                self.db
                    .execute("roll back a savepoint", "ROLLBACK TO sm_step", [])?;
                self.db
                    .execute("release a savepoint", "RELEASE sm_step", [])?;
                self.wrote = wrote;
                self.changes = changes;
                // The step may have cached routing it then rolled back.
                self.routing.clear();
                Err(e)
            }
        }
    }

    /// Makes this transaction's commit fail, as a full disk or a broken
    /// deferred constraint would, for tests of what follows a failed commit.
    #[cfg(any(test, feature = "test-hooks"))]
    pub const fn fail_commit(&mut self) {
        self.fail_commit = true;
    }

    pub(crate) const fn db(&self) -> Db<'c> {
        self.db
    }

    /// Marks the transaction as writing and returns the `rev` to stamp.
    pub(crate) const fn stamp(&mut self) -> Rev {
        self.wrote = true;
        self.rev
    }

    pub(crate) const fn record(&mut self) -> &mut Changes {
        self.wrote = true;
        &mut self.changes
    }

    /// Records a write to `account`'s `table`.
    pub(crate) fn touch(&mut self, account: AccountId, table: Table) {
        self.record().touch(account, table);
    }

    pub(crate) const fn routing_cache(&mut self) -> &mut HashMap<AccountId, Routing> {
        &mut self.routing
    }

    /// Drops the cached routing of an account whose splits, pins, labels, or
    /// addresses changed.
    pub(crate) fn forget_routing(&mut self, account: AccountId) {
        self.routing.remove(&account);
    }

    /// Whether a row changed, and what.
    pub(crate) fn finish(self) -> Finished {
        Finished {
            wrote: self.wrote,
            changes: self.changes,
            #[cfg(any(test, feature = "test-hooks"))]
            fail_commit: self.fail_commit,
        }
    }
}

/// A transaction's outcome, before its commit.
pub(crate) struct Finished {
    pub(crate) wrote: bool,
    pub(crate) changes: Changes,
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fail_commit: bool,
}

/// The read-only repositories, over one connection. [`WriteTx::read`],
/// [`Snapshot`], and [`ReadTx`] all give one.
#[derive(Debug, Clone, Copy)]
pub struct Reads<'c> {
    db: Db<'c>,
}

impl<'c> Reads<'c> {
    pub(crate) const fn new(conn: &'c Connection) -> Self {
        Self { db: Db::new(conn) }
    }

    /// The last action number and `rev`.
    #[must_use]
    pub const fn meta(self) -> MetaReads<'c> {
        MetaReads::new(self.db)
    }

    /// Accounts.
    #[must_use]
    pub const fn accounts(self) -> AccountReads<'c> {
        AccountReads::new(self.db)
    }

    /// Labels.
    #[must_use]
    pub const fn labels(self) -> LabelReads<'c> {
        LabelReads::new(self.db)
    }

    /// Thread rows and view lists.
    #[must_use]
    pub const fn threads(self) -> ThreadReads<'c> {
        ThreadReads::new(self.db)
    }

    /// Messages, bodies, and attachments.
    #[must_use]
    pub const fn messages(self) -> MessageReads<'c> {
        MessageReads::new(self.db)
    }

    /// Drafts.
    #[must_use]
    pub const fn drafts(self) -> DraftReads<'c> {
        DraftReads::new(self.db)
    }

    /// The outbox.
    #[must_use]
    pub const fn outbox(self) -> OutboxReads<'c> {
        OutboxReads::new(self.db)
    }

    /// The Gmail op queue.
    #[must_use]
    pub const fn ops(self) -> OpReads<'c> {
        OpReads::new(self.db)
    }

    /// The undo history.
    #[must_use]
    pub const fn undo(self) -> UndoReads<'c> {
        UndoReads::new(self.db)
    }

    /// Notices.
    #[must_use]
    pub const fn notices(self) -> NoticeReads<'c> {
        NoticeReads::new(self.db)
    }

    /// Splits and pins.
    #[must_use]
    pub const fn splits(self) -> SplitReads<'c> {
        SplitReads::new(self.db)
    }

    /// Contacts for autocomplete.
    #[must_use]
    pub const fn contacts(self) -> ContactReads<'c> {
        ContactReads::new(self.db)
    }

    /// Settings.
    #[must_use]
    pub const fn settings(self) -> SettingReads<'c> {
        SettingReads::new(self.db)
    }

    /// Local search.
    #[must_use]
    pub const fn search(self) -> SearchReads<'c> {
        SearchReads::new(self.db)
    }
}

/// The database as the writer just committed it, read on the write
/// connection: what `on_commit` builds first loads and patches from.
#[derive(Debug)]
pub struct Snapshot<'c> {
    reads: Reads<'c>,
}

impl<'c> Snapshot<'c> {
    pub(crate) const fn new(conn: &'c Connection) -> Self {
        Self {
            reads: Reads::new(conn),
        }
    }
}

impl<'c> Deref for Snapshot<'c> {
    type Target = Reads<'c>;

    fn deref(&self) -> &Reads<'c> {
        &self.reads
    }
}

/// A read transaction on a read pool connection: every read in it sees the
/// same committed state.
#[derive(Debug)]
pub struct ReadTx<'c> {
    reads: Reads<'c>,
}

impl<'c> ReadTx<'c> {
    pub(crate) const fn new(conn: &'c Connection) -> Self {
        Self {
            reads: Reads::new(conn),
        }
    }
}

impl<'c> Deref for ReadTx<'c> {
    type Target = Reads<'c>;

    fn deref(&self) -> &Reads<'c> {
        &self.reads
    }
}

/// A missing row named by a write.
pub(crate) fn require<T>(
    value: Option<T>,
    what: &'static str,
    id: &dyn fmt::Display,
) -> Result<T, StoreError> {
    value.ok_or_else(|| StoreError::not_found(what, id))
}
