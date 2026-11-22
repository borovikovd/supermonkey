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

pub struct WriteTx<'c> {
    db: Db<'c>,
    rev: Rev,
    wrote: bool,
    changes: Changes,
    routing: HashMap<AccountId, Routing>,
    #[cfg(any(test, feature = "test-hooks"))]
    fail_commit: bool,
}

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

    #[must_use]
    pub const fn rev(&self) -> Rev {
        self.rev
    }

    #[must_use]
    pub const fn read(&self) -> Reads<'c> {
        Reads { db: self.db }
    }

    pub const fn meta(&mut self) -> MetaWrites<'_, 'c> {
        MetaWrites::new(self)
    }

    pub const fn accounts(&mut self) -> AccountWrites<'_, 'c> {
        AccountWrites::new(self)
    }

    pub const fn labels(&mut self) -> LabelWrites<'_, 'c> {
        LabelWrites::new(self)
    }

    pub const fn threads(&mut self) -> ThreadWrites<'_, 'c> {
        ThreadWrites::new(self)
    }

    pub const fn messages(&mut self) -> MessageWrites<'_, 'c> {
        MessageWrites::new(self)
    }

    pub const fn drafts(&mut self) -> DraftWrites<'_, 'c> {
        DraftWrites::new(self)
    }

    pub const fn outbox(&mut self) -> OutboxWrites<'_, 'c> {
        OutboxWrites::new(self)
    }

    pub const fn ops(&mut self) -> OpWrites<'_, 'c> {
        OpWrites::new(self)
    }

    pub const fn undo(&mut self) -> UndoWrites<'_, 'c> {
        UndoWrites::new(self)
    }

    pub const fn notices(&mut self) -> NoticeWrites<'_, 'c> {
        NoticeWrites::new(self)
    }

    pub const fn splits(&mut self) -> SplitWrites<'_, 'c> {
        SplitWrites::new(self)
    }

    pub const fn contacts(&mut self) -> ContactWrites<'_, 'c> {
        ContactWrites::new(self)
    }

    pub const fn settings(&mut self) -> SettingWrites<'_, 'c> {
        SettingWrites::new(self)
    }

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
                self.routing.clear();
                Err(e)
            }
        }
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub const fn fail_commit(&mut self) {
        self.fail_commit = true;
    }

    pub(crate) const fn db(&self) -> Db<'c> {
        self.db
    }

    pub(crate) const fn stamp(&mut self) -> Rev {
        self.wrote = true;
        self.rev
    }

    pub(crate) const fn record(&mut self) -> &mut Changes {
        self.wrote = true;
        &mut self.changes
    }

    pub(crate) fn touch(&mut self, account: AccountId, table: Table) {
        self.record().touch(account, table);
    }

    pub(crate) const fn routing_cache(&mut self) -> &mut HashMap<AccountId, Routing> {
        &mut self.routing
    }

    pub(crate) fn forget_routing(&mut self, account: AccountId) {
        self.routing.remove(&account);
    }

    pub(crate) fn finish(self) -> Finished {
        Finished {
            wrote: self.wrote,
            changes: self.changes,
            #[cfg(any(test, feature = "test-hooks"))]
            fail_commit: self.fail_commit,
        }
    }
}

pub(crate) struct Finished {
    pub(crate) wrote: bool,
    pub(crate) changes: Changes,
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fail_commit: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Reads<'c> {
    db: Db<'c>,
}

impl<'c> Reads<'c> {
    pub(crate) const fn new(conn: &'c Connection) -> Self {
        Self { db: Db::new(conn) }
    }

    #[must_use]
    pub const fn meta(self) -> MetaReads<'c> {
        MetaReads::new(self.db)
    }

    #[must_use]
    pub const fn accounts(self) -> AccountReads<'c> {
        AccountReads::new(self.db)
    }

    #[must_use]
    pub const fn labels(self) -> LabelReads<'c> {
        LabelReads::new(self.db)
    }

    #[must_use]
    pub const fn threads(self) -> ThreadReads<'c> {
        ThreadReads::new(self.db)
    }

    #[must_use]
    pub const fn messages(self) -> MessageReads<'c> {
        MessageReads::new(self.db)
    }

    #[must_use]
    pub const fn drafts(self) -> DraftReads<'c> {
        DraftReads::new(self.db)
    }

    #[must_use]
    pub const fn outbox(self) -> OutboxReads<'c> {
        OutboxReads::new(self.db)
    }

    #[must_use]
    pub const fn ops(self) -> OpReads<'c> {
        OpReads::new(self.db)
    }

    #[must_use]
    pub const fn undo(self) -> UndoReads<'c> {
        UndoReads::new(self.db)
    }

    #[must_use]
    pub const fn notices(self) -> NoticeReads<'c> {
        NoticeReads::new(self.db)
    }

    #[must_use]
    pub const fn splits(self) -> SplitReads<'c> {
        SplitReads::new(self.db)
    }

    #[must_use]
    pub const fn contacts(self) -> ContactReads<'c> {
        ContactReads::new(self.db)
    }

    #[must_use]
    pub const fn settings(self) -> SettingReads<'c> {
        SettingReads::new(self.db)
    }

    #[must_use]
    pub const fn search(self) -> SearchReads<'c> {
        SearchReads::new(self.db)
    }
}

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

pub(crate) fn require<T>(
    value: Option<T>,
    what: &'static str,
    id: &dyn fmt::Display,
) -> Result<T, StoreError> {
    value.ok_or_else(|| StoreError::not_found(what, id))
}
