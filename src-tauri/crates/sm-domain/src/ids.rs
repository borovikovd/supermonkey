use std::borrow::Borrow;
use std::fmt;
use std::num::ParseIntError;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

macro_rules! int_id {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = ParseIntError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse().map(Self)
            }
        }

        #[cfg(feature = "sqlite")]
        impl rusqlite::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }

        #[cfg(feature = "sqlite")]
        impl rusqlite::types::FromSql for $name {
            fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
                i64::column_result(v).map(Self)
            }
        }

        #[cfg(feature = "ts")]
        impl ts_rs::TS for $name {
            type WithoutGenerics = Self;
            type OptionInnerType = Self;
            fn name(_: &ts_rs::Config) -> String {
                "number".to_owned()
            }
            fn inline(_: &ts_rs::Config) -> String {
                "number".to_owned()
            }
        }
    )*};
}

macro_rules! str_id {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
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

        #[cfg(feature = "sqlite")]
        impl rusqlite::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }

        #[cfg(feature = "sqlite")]
        impl rusqlite::types::FromSql for $name {
            fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
                String::column_result(v).map(Self)
            }
        }

        #[cfg(feature = "ts")]
        impl ts_rs::TS for $name {
            type WithoutGenerics = Self;
            type OptionInnerType = Self;
            fn name(_: &ts_rs::Config) -> String {
                "string".to_owned()
            }
            fn inline(_: &ts_rs::Config) -> String {
                "string".to_owned()
            }
        }
    )*};
}

int_id!(
    AccountId,
    ThreadId,
    MessageId,
    SplitId,
    OutboxId,
    UndoEntryId,
    NoticeId,
    AttachmentId,
    Rev,
    ActionNo,
    OpId,
);

impl AccountId {
    pub const NONE: Self = Self(0);
}

impl SplitId {
    #[must_use]
    pub const fn is_saved(self) -> bool {
        self.0 > 0
    }
}

str_id!(
    DraftId,
    SubId,
    LabelId,
    GmailThreadId,
    GmailMessageId,
    GmailDraftId,
    GmailAttachmentId,
    PageToken,
    FilterId,
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct HistoryId(pub u64);

impl fmt::Display for HistoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for HistoryId {
    type Err = ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

impl<'de> Deserialize<'de> for HistoryId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(u64),
            Text(String),
        }
        match Raw::deserialize(d)? {
            Raw::Number(n) => Ok(Self(n)),
            Raw::Text(s) => s.parse().map_err(D::Error::custom),
        }
    }
}

#[cfg(feature = "sqlite")]
impl rusqlite::ToSql for HistoryId {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(self.0.to_string().into())
    }
}

#[cfg(feature = "sqlite")]
impl rusqlite::types::FromSql for HistoryId {
    fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        v.as_str()?
            .parse()
            .map_err(|e| rusqlite::types::FromSqlError::Other(Box::new(e)))
    }
}
