//! The mailbox DTO.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// One mailbox: a display name and an address. Its own type, not
// `ub_domain::Addr`, because that one's doc comment would land in the binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Addr {
    pub name: Option<String>,
    pub email: String,
}

impl From<ub_domain::Addr> for Addr {
    fn from(addr: ub_domain::Addr) -> Self {
        Self {
            name: addr.name,
            email: addr.email,
        }
    }
}

impl From<Addr> for ub_domain::Addr {
    fn from(addr: Addr) -> Self {
        Self {
            name: addr.name,
            email: addr.email,
        }
    }
}
