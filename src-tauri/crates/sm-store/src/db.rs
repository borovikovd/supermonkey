//! Statement helpers that attach what was being done to SQLite's errors, and
//! JSON columns.
//!
//! Every SQL string is a `&'static str` literal next to the repository method
//! that runs it, so none is built at runtime. The one dynamic value, the FTS5
//! query, is always bound as a parameter.

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension as _, Params, Row};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::StoreError;

/// A connection, with helpers that run one cached statement each.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Db<'c> {
    conn: &'c Connection,
}

impl<'c> Db<'c> {
    pub(crate) const fn new(conn: &'c Connection) -> Self {
        Self { conn }
    }

    /// Runs a write; returns the number of rows it changed.
    pub(crate) fn execute(
        self,
        what: &'static str,
        sql: &'static str,
        params: impl Params,
    ) -> Result<usize, StoreError> {
        self.conn
            .prepare_cached(sql)
            .and_then(|mut stmt| stmt.execute(params))
            .map_err(StoreError::sql(what))
    }

    /// Runs an insert; returns the new row's id.
    pub(crate) fn insert(
        self,
        what: &'static str,
        sql: &'static str,
        params: impl Params,
    ) -> Result<i64, StoreError> {
        self.execute(what, sql, params)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The first row, if any.
    pub(crate) fn optional<T>(
        self,
        what: &'static str,
        sql: &'static str,
        params: impl Params,
        map: impl FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Option<T>, StoreError> {
        self.conn
            .prepare_cached(sql)
            .and_then(|mut stmt| stmt.query_row(params, map).optional())
            .map_err(StoreError::sql(what))
    }

    /// The first row; an error when there is none.
    pub(crate) fn one<T>(
        self,
        what: &'static str,
        sql: &'static str,
        params: impl Params,
        map: impl FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<T, StoreError> {
        self.conn
            .prepare_cached(sql)
            .and_then(|mut stmt| stmt.query_row(params, map))
            .map_err(StoreError::sql(what))
    }

    /// Every row, in the statement's order.
    pub(crate) fn all<T>(
        self,
        what: &'static str,
        sql: &'static str,
        params: impl Params,
        map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>, StoreError> {
        self.conn
            .prepare_cached(sql)
            .and_then(|mut stmt| stmt.query_map(params, map)?.collect())
            .map_err(StoreError::sql(what))
    }
}

/// A JSON column read as `T`. A value that does not parse is an error, not a
/// default: the store wrote it, so a bad one means a bug to surface.
pub(crate) fn json_column<T: DeserializeOwned>(row: &Row<'_>, column: &str) -> rusqlite::Result<T> {
    let index = row.as_ref().column_index(column)?;
    let text = row.get_ref(index)?.as_str()?;
    serde_json::from_str(text)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(e)))
}

/// A nullable JSON column read as `T`; see [`json_column`].
pub(crate) fn json_column_opt<T: DeserializeOwned>(
    row: &Row<'_>,
    column: &str,
) -> rusqlite::Result<Option<T>> {
    let index = row.as_ref().column_index(column)?;
    row.get_ref(index)?
        .as_str_or_null()?
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(e)))
}

/// A value as a JSON column's text.
pub(crate) fn to_json<T: Serialize + ?Sized>(
    column: &'static str,
    value: &T,
) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|source| StoreError::Json { column, source })
}

/// Ids as a JSON array, for `json_each(?)`: one bound parameter in place of
/// an `IN` list built at runtime. Its `key` keeps the input order.
pub(crate) fn id_list<T: Serialize>(ids: &[T]) -> Result<String, StoreError> {
    to_json("id list", ids)
}
