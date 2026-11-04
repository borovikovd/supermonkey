//! Gmail's ids that `ub-domain` has no newtype for: they live only between
//! Gmail and the core, which converts them to raw values for SQL.

use std::fmt;

use serde::{Deserialize, Deserializer};

/// Gmail's mailbox history cursor (`historyId`). Gmail sends it as a decimal
/// string; it grows with every change to the mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HistoryId(pub u64);

impl fmt::Display for HistoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for HistoryId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        string_u64(d).map(Self)
    }
}

/// Reads a `u64` that Gmail encodes as a JSON string (or, leniently, a number).
pub(crate) fn string_u64<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    use serde::de::Error as _;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Number(u64),
        Text(String),
    }
    match Raw::deserialize(d)? {
        Raw::Number(n) => Ok(n),
        Raw::Text(s) => s.parse().map_err(D::Error::custom),
    }
}

macro_rules! opaque_id {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// The id as Gmail spells it.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_owned())
            }
        }
    )*};
}

opaque_id!(
    /// A list call's cursor to its next page (`nextPageToken`).
    PageToken,
    /// Gmail's id of a message attachment (`body.attachmentId`).
    GmailAttachmentId,
    /// Gmail's id of a settings filter.
    FilterId,
);
