//! Who an account is: its address and the `sendAs` addresses it may send as.

use serde::{Deserialize, Serialize};

use crate::Addr;

/// One of Gmail's `sendAs` addresses for an account. Its JSON form is what
/// the account row stores: renaming a field changes the stored format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendAsAddress {
    /// The address.
    pub email: String,
    /// Its display name; empty for none.
    pub name: String,
    /// Gmail's default for new mail.
    pub is_default: bool,
    /// The account's own address.
    pub is_primary: bool,
}

impl SendAsAddress {
    /// The address as a mailbox, with no display name when it has none.
    #[must_use]
    pub fn addr(&self) -> Addr {
        Addr {
            name: (!self.name.is_empty()).then(|| self.name.clone()),
            email: self.email.clone(),
        }
    }
}

/// An account's identity: its sign-in address and display name, and its
/// `sendAs` list. Decides what counts as mail "from me".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    email: String,
    name: String,
    send_as: Vec<SendAsAddress>,
    /// Every address above, lowercased, for [`Identity::is_mine`].
    keys: Vec<String>,
}

impl Identity {
    /// The identity of an account signed in as `email`, shown as `name`.
    #[must_use]
    pub fn new(email: String, name: String, send_as: Vec<SendAsAddress>) -> Self {
        let mut keys: Vec<String> = send_as.iter().map(|s| s.email.to_lowercase()).collect();
        keys.push(email.to_lowercase());
        keys.sort_unstable();
        keys.dedup();
        Self {
            email,
            name,
            send_as,
            keys,
        }
    }

    /// The sign-in address.
    #[must_use]
    pub fn email(&self) -> &str {
        &self.email
    }

    /// The `sendAs` addresses, as Gmail lists them.
    #[must_use]
    pub fn send_as(&self) -> &[SendAsAddress] {
        &self.send_as
    }

    /// Whether `email` is one of this account's addresses, ignoring case.
    #[must_use]
    pub fn is_mine(&self, email: &str) -> bool {
        let key = email.to_lowercase();
        self.keys.binary_search(&key).is_ok()
    }

    /// The `sendAs` entry for `email`, ignoring case.
    #[must_use]
    pub fn send_as_for(&self, email: &str) -> Option<&SendAsAddress> {
        self.send_as
            .iter()
            .find(|s| s.email.to_lowercase() == email.to_lowercase())
    }

    /// The From of new mail and forwards: Gmail's default `sendAs`, else the
    /// primary one, else the sign-in address (named only when the display
    /// name is not just the address again).
    pub fn default_from(&self) -> Addr {
        self.send_as
            .iter()
            .find(|s| s.is_default)
            .or_else(|| self.send_as.iter().find(|s| s.is_primary))
            .map_or_else(
                || Addr {
                    name: (!self.name.is_empty() && self.name != self.email)
                        .then(|| self.name.clone()),
                    email: self.email.clone(),
                },
                SendAsAddress::addr,
            )
    }
}
