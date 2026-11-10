//! The mailbox DTO.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One mailbox: a display name and an address.
// Its own type, not `sm_domain::Addr`, because that one's doc comment would
// land in the binding.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Addr {
    /// The display name. `None` when the header gives only an address.
    pub name: Option<String>,
    /// The address.
    pub email: String,
}

impl From<sm_domain::Addr> for Addr {
    fn from(addr: sm_domain::Addr) -> Self {
        Self {
            name: addr.name,
            email: addr.email,
        }
    }
}

impl From<Addr> for sm_domain::Addr {
    fn from(addr: Addr) -> Self {
        Self {
            name: addr.name,
            email: addr.email,
        }
    }
}
