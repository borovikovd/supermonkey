//! Points in time.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Milliseconds since the Unix epoch. TypeScript sees `number`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Millis(pub i64);

impl Millis {
    /// `ms` milliseconds later.
    #[must_use]
    pub fn plus(self, ms: i64) -> Self {
        Self(self.0.saturating_add(ms))
    }

    /// Milliseconds from `earlier` to `self`, negative if `earlier` is later.
    pub fn since(self, earlier: Self) -> i64 {
        self.0.saturating_sub(earlier.0)
    }
}

impl fmt::Display for Millis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(feature = "sqlite")]
impl rusqlite::ToSql for Millis {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

#[cfg(feature = "sqlite")]
impl rusqlite::types::FromSql for Millis {
    fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        i64::column_result(v).map(Self)
    }
}

#[cfg(feature = "ts")]
impl ts_rs::TS for Millis {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;
    fn name(_: &ts_rs::Config) -> String {
        "number".to_owned()
    }
    fn inline(_: &ts_rs::Config) -> String {
        "number".to_owned()
    }
}
