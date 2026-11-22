use std::fmt;

use serde::{Deserialize, Deserializer};

pub use sm_domain::HistoryId;

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

opaque_id!(PageToken, GmailAttachmentId, FilterId,);
