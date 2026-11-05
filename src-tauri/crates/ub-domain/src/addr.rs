//! Mailboxes: a display name and an address.

use serde::{Deserialize, Serialize};

/// One mailbox, as in a `From` or `To` header. Parsing and formatting header
/// values is `ub-mime`'s job; this is only the value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Addr {
    pub name: Option<String>,
    pub email: String,
}

impl Addr {
    /// An address with no display name.
    pub fn new(email: impl Into<String>) -> Self {
        Self {
            name: None,
            email: email.into(),
        }
    }

    /// An address with a display name.
    pub fn named(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            email: email.into(),
        }
    }

    /// The address lowercased, for comparing two mailboxes.
    pub fn key(&self) -> String {
        self.email.to_lowercase()
    }
}
