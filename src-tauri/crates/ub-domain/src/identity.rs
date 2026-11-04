//! Who an account is: its address and the `sendAs` addresses it may send as.

use crate::Addr;

/// One of Gmail's `sendAs` addresses for an account.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub fn email(&self) -> &str {
        &self.email
    }

    /// The `sendAs` addresses, as Gmail lists them.
    pub fn send_as(&self) -> &[SendAsAddress] {
        &self.send_as
    }

    /// Whether `email` is one of this account's addresses, ignoring case.
    pub fn is_mine(&self, email: &str) -> bool {
        let key = email.to_lowercase();
        self.keys.binary_search(&key).is_ok()
    }

    /// The `sendAs` entry for `email`, ignoring case.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn send_as(email: &str, name: &str, is_default: bool, is_primary: bool) -> SendAsAddress {
        SendAsAddress {
            email: email.into(),
            name: name.into(),
            is_default,
            is_primary,
        }
    }

    #[test]
    fn mine_covers_sign_in_and_send_as_ignoring_case() {
        let me = Identity::new(
            "Me@Example.com".into(),
            "Me".into(),
            vec![send_as("alias@corp.io", "", false, false)],
        );
        assert!(me.is_mine("me@example.com"));
        assert!(me.is_mine("ALIAS@corp.io"));
        assert!(!me.is_mine("other@example.com"));
    }

    #[test]
    fn default_from_prefers_default_then_primary_then_sign_in() {
        let both = Identity::new(
            "me@x.io".into(),
            "Me".into(),
            vec![
                send_as("me@x.io", "Primary", false, true),
                send_as("work@x.io", "Work", true, false),
            ],
        );
        assert_eq!(both.default_from(), Addr::named("Work", "work@x.io"));
        let primary = Identity::new(
            "me@x.io".into(),
            "Me".into(),
            vec![send_as("me@x.io", "", false, true)],
        );
        assert_eq!(primary.default_from(), Addr::new("me@x.io"));
        let none = Identity::new("me@x.io".into(), "Me Myself".into(), vec![]);
        assert_eq!(none.default_from(), Addr::named("Me Myself", "me@x.io"));
        let echo = Identity::new("me@x.io".into(), "me@x.io".into(), vec![]);
        assert_eq!(echo.default_from(), Addr::new("me@x.io"));
    }
}
