use std::collections::HashMap;

use parking_lot::Mutex;
use sm_domain::AccountId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Grant(u64);

#[derive(Debug, Default)]
pub(crate) struct Grants(Mutex<GrantTable>);

#[derive(Debug, Default)]
struct GrantTable {
    current: HashMap<AccountId, Grant>,
    last: u64,
}

impl Grants {
    pub(crate) fn current(&self, account: AccountId) -> Grant {
        self.0
            .lock()
            .current
            .get(&account)
            .copied()
            .unwrap_or_default()
    }

    pub(crate) fn renew(&self, account: AccountId) -> Grant {
        let mut table = self.0.lock();
        table.last = table.last.saturating_add(1);
        let grant = Grant(table.last);
        table.current.insert(account, grant);
        grant
    }
}
