use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension as _, Params, Row};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::StoreError;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Db<'c> {
    conn: &'c Connection,
}

impl<'c> Db<'c> {
    pub(crate) const fn new(conn: &'c Connection) -> Self {
        Self { conn }
    }

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

    pub(crate) fn insert(
        self,
        what: &'static str,
        sql: &'static str,
        params: impl Params,
    ) -> Result<i64, StoreError> {
        self.execute(what, sql, params)?;
        Ok(self.conn.last_insert_rowid())
    }

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

pub(crate) fn json_column<T: DeserializeOwned>(row: &Row<'_>, column: &str) -> rusqlite::Result<T> {
    let index = row.as_ref().column_index(column)?;
    let text = row.get_ref(index)?.as_str()?;
    serde_json::from_str(text)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(e)))
}

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

pub(crate) fn to_json<T: Serialize + ?Sized>(
    column: &'static str,
    value: &T,
) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|source| StoreError::Json { column, source })
}

pub(crate) fn id_list<T: Serialize>(ids: &[T]) -> Result<String, StoreError> {
    to_json("id list", ids)
}
