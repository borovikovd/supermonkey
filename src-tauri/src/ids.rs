//! One newtype per kind of id, so the compiler rejects an `AccountId` where a
//! `ThreadId` is expected. On the wire (serde, SQLite, TypeScript) each is
//! exactly its inner value.

use std::borrow::Borrow;
use std::fmt;
use std::num::ParseIntError;
use std::str::FromStr;

use rusqlite::types::{FromSql, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Row ids. TypeScript sees `number`: ids stay far below 2^53.
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

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }

        impl FromSql for $name {
            fn column_result(v: ValueRef<'_>) -> FromSqlResult<Self> {
                i64::column_result(v).map(Self)
            }
        }

        // Written by hand: the derive would emit a named alias and an import
        // into every binding, where the bindings want a plain `number`.
        impl TS for $name {
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

        // Lets maps keyed by the id be looked up with a `&str`.
        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }

        impl FromSql for $name {
            fn column_result(v: ValueRef<'_>) -> FromSqlResult<Self> {
                String::column_result(v).map(Self)
            }
        }

        impl TS for $name {
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
);

impl AccountId {
    /// Rowids start at 1, so 0 is no account: it keys the global settings and
    /// stands in for the owner of a row that is gone.
    pub const NONE: Self = Self(0);
}

impl SplitId {
    /// The settings sheet numbers the splits it has not saved yet below 1.
    pub fn is_saved(self) -> bool {
        self.0 > 0
    }
}

str_id!(
    /// A local draft's id. Not Gmail's draft id, which stays a `String`.
    DraftId,
    /// A UI subscription's id.
    SubId,
);
