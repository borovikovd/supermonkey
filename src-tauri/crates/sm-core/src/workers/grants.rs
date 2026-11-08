//! Which sign-in each account's grant comes from, so a sign-out reported
//! for an older grant never undoes a newer sign-in (SPEC Accounts).
//!
//! A worker that gets `invalid_grant` reports it, and the account is marked
//! signed out in a writer job queued behind the report. A sign-in meanwhile
//! marks it signed in again in a writer job of its own, which may run first
//! (it has the UI's priority). So every sign-in renews the account's grant
//! inside that job, and the sign-out's job applies only while the grant it
//! names is still the account's: both checks run on the writer thread, in
//! commit order.

use std::collections::HashMap;

use parking_lot::Mutex;
use sm_domain::AccountId;

/// One sign-in's grant of an account. Numbers never repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Grant(u64);

/// Every account's current grant.
#[derive(Debug, Default)]
pub(crate) struct Grants(Mutex<GrantTable>);

#[derive(Debug, Default)]
struct GrantTable {
    current: HashMap<AccountId, Grant>,
    last: u64,
}

impl Grants {
    /// The account's grant now: the one its last sign-in made, or the
    /// stored token's before any sign-in this run.
    pub(crate) fn current(&self, account: AccountId) -> Grant {
        self.0
            .lock()
            .current
            .get(&account)
            .copied()
            .unwrap_or_default()
    }

    /// A sign-in replaced the account's grant. Call it in the writer job
    /// that marks the account signed in.
    pub(crate) fn renew(&self, account: AccountId) -> Grant {
        let mut table = self.0.lock();
        table.last = table.last.saturating_add(1);
        let grant = Grant(table.last);
        table.current.insert(account, grant);
        grant
    }
}
