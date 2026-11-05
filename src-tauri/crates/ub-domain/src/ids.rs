//! One newtype per kind of id, so an `AccountId` can't be passed where a
//! `ThreadId` is meant. On the wire (serde, SQLite, TypeScript) each is exactly
//! its inner value.

use std::borrow::Borrow;
use std::fmt;
use std::num::ParseIntError;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Integer ids. TypeScript sees `number`: they stay far below 2^53.
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

        // List keys carry thread ids as text.
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

        // By hand: the derive would emit a named alias and an import into
        // every binding, where the bindings want a plain `number`.
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

/// Opaque string ids. TypeScript sees `string`.
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

        // Maps keyed by the id can be looked up with a `&str`.
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
    /// An account's row.
    AccountId,
    /// A thread's row; not Gmail's thread id.
    ThreadId,
    /// A message's row; not Gmail's message id.
    MessageId,
    /// A Split Inbox split.
    SplitId,
    /// An outbox row: one send waiting, sending, or sent.
    OutboxId,
    /// An undo history entry: the number of the action that made it.
    UndoEntryId,
    /// A notice waiting for the UI to show and ack it.
    NoticeId,
    /// An attachment of a stored message.
    AttachmentId,
    /// The `rev` a write transaction stamps on the rows it changes (SPEC UI state).
    Rev,
    /// A UI action's number; the channel's `ack` is the highest one applied.
    ActionNo,
);

impl AccountId {
    /// Rowids start at 1, so 0 is no account: it keys the global settings and
    /// stands for the owner of a row that is gone.
    pub const NONE: Self = Self(0);
}

impl SplitId {
    /// The Split Inbox sheet numbers splits it has not saved yet below 1.
    pub fn is_saved(self) -> bool {
        self.0 > 0
    }
}

str_id!(
    /// A local draft's id, made by the UI; not Gmail's draft id.
    DraftId,
    /// A UI subscription's id, made by the UI.
    SubId,
    /// A label's local id: Gmail's for labels Gmail made, the UI's for labels
    /// made here until Gmail assigns one (SPEC Labels).
    LabelId,
    /// Gmail's id of a thread.
    GmailThreadId,
    /// Gmail's id of a message.
    GmailMessageId,
    /// Gmail's id of a draft.
    GmailDraftId,
);

/// Gmail's mailbox history cursor (`historyId`): it grows with every change
/// to the mailbox. Gmail sends it as a decimal string, and SQLite keeps it
/// as one; it is a number everywhere else.
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

/// Gmail's JSON spelling: a decimal string or, leniently, a number.
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

/// A stored cursor that is not a number is an error, never a missing one:
/// polling would stop without a word.
#[cfg(feature = "sqlite")]
impl rusqlite::types::FromSql for HistoryId {
    fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        v.as_str()?
            .parse()
            .map_err(|e| rusqlite::types::FromSqlError::Other(Box::new(e)))
    }
}
