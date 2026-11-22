use serde::{Deserialize, Serialize};

use crate::Addr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendAsAddress {
    pub email: String,
    pub name: String,
    pub is_default: bool,
    pub is_primary: bool,
}

impl SendAsAddress {
    #[must_use]
    pub fn addr(&self) -> Addr {
        Addr {
            name: (!self.name.is_empty()).then(|| self.name.clone()),
            email: self.email.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    email: String,
    name: String,
    send_as: Vec<SendAsAddress>,
    keys: Vec<String>,
}

impl Identity {
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

    #[must_use]
    pub fn is_mine(&self, email: &str) -> bool {
        let key = email.to_lowercase();
        self.keys.binary_search(&key).is_ok()
    }

    #[must_use]
    pub fn send_as_for(&self, email: &str) -> Option<&SendAsAddress> {
        self.send_as
            .iter()
            .find(|s| s.email.to_lowercase() == email.to_lowercase())
    }

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
