use std::str::FromStr as _;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::{Connection, OptionalExtension as _, Params, Row, ToSql};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::domain::{
    AccountId, ActionNo, AttachmentId, DraftId, DraftKind, DraftWait, GmailAttachmentId,
    GmailDraftId, GmailMessageId, GmailThreadId, HistoryId, LabelId, MessageId, Millis, ModifyKind,
    NoticeId, NoticeKind, OpId, OpKind, OutboxId, OutboxState, PinKind, Rev, SettingKey, SplitId,
    SplitKind, SyncStatus, ThreadId, UndoEntryId,
};

use super::StoreError;

macro_rules! sql_int_id {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl ToSql for $ty {
                fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                    Ok(ToSqlOutput::from(self.0))
                }
            }

            impl FromSql for $ty {
                fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                    i64::column_result(value).map(Self)
                }
            }
        )+
    };
}

macro_rules! sql_str_id {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl ToSql for $ty {
                fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                    Ok(ToSqlOutput::from(self.0.as_str()))
                }
            }

            impl FromSql for $ty {
                fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                    String::column_result(value).map(Self)
                }
            }
        )+
    };
}

macro_rules! sql_text_enum {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl ToSql for $ty {
                fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                    Ok(ToSqlOutput::from(self.as_str()))
                }
            }

            impl FromSql for $ty {
                fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                    let text = value.as_str()?;
                    Self::from_str(text).map_err(|e| FromSqlError::Other(Box::new(e)))
                }
            }
        )+
    };
}

sql_int_id!(
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
    Millis
);

sql_str_id!(
    DraftId,
    LabelId,
    GmailThreadId,
    GmailMessageId,
    GmailDraftId,
    GmailAttachmentId
);

sql_text_enum!(
    DraftKind,
    OutboxState,
    SplitKind,
    PinKind,
    SyncStatus,
    NoticeKind,
    ModifyKind,
    SettingKey,
    OpKind,
    DraftWait
);

impl ToSql for HistoryId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Owned(rusqlite::types::Value::Text(
            self.0.to_string(),
        )))
    }
}

impl FromSql for HistoryId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Text(bytes) => {
                let s = std::str::from_utf8(bytes).map_err(|e| FromSqlError::Other(Box::new(e)))?;
                s.parse::<u64>()
                    .map(Self)
                    .map_err(|e| FromSqlError::Other(Box::new(e)))
            }
            ValueRef::Integer(n) => u64::try_from(n)
                .map(Self)
                .map_err(|e| FromSqlError::Other(Box::new(e))),
            ValueRef::Null | ValueRef::Real(_) | ValueRef::Blob(_) => {
                Err(FromSqlError::InvalidType)
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct Db<'c>(pub(super) &'c Connection);

impl std::fmt::Debug for Db<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db").finish_non_exhaustive()
    }
}

impl Db<'_> {
    pub(super) fn execute(
        self,
        sql: &str,
        params: impl Params,
        what: &'static str,
    ) -> Result<usize, StoreError> {
        let mut stmt = self.0.prepare_cached(sql).map_err(StoreError::sql(what))?;
        stmt.execute(params).map_err(StoreError::sql(what))
    }

    pub(super) fn insert(
        self,
        sql: &str,
        params: impl Params,
        what: &'static str,
    ) -> Result<i64, StoreError> {
        self.execute(sql, params, what)?;
        Ok(self.0.last_insert_rowid())
    }

    pub(super) fn optional<T>(
        self,
        sql: &str,
        params: impl Params,
        what: &'static str,
        map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Option<T>, StoreError> {
        let mut stmt = self.0.prepare_cached(sql).map_err(StoreError::sql(what))?;
        stmt.query_row(params, map)
            .optional()
            .map_err(StoreError::sql(what))
    }

    pub(super) fn one<T>(
        self,
        sql: &str,
        params: impl Params,
        what: &'static str,
        map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<T, StoreError> {
        let mut stmt = self.0.prepare_cached(sql).map_err(StoreError::sql(what))?;
        stmt.query_row(params, map).map_err(StoreError::sql(what))
    }

    pub(super) fn all<T>(
        self,
        sql: &str,
        params: impl Params,
        what: &'static str,
        map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>, StoreError> {
        let mut stmt = self.0.prepare_cached(sql).map_err(StoreError::sql(what))?;
        let rows = stmt.query_map(params, map).map_err(StoreError::sql(what))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(StoreError::sql(what))?);
        }
        Ok(out)
    }
}

pub(super) fn to_json<T: Serialize + ?Sized>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(StoreError::Json)
}

pub(super) fn json_col<T: DeserializeOwned>(row: &Row<'_>, idx: usize) -> rusqlite::Result<T> {
    let raw: String = row.get(idx)?;
    serde_json::from_str(&raw).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
    })
}

pub(super) fn json_col_opt<T: DeserializeOwned>(
    row: &Row<'_>,
    idx: usize,
) -> rusqlite::Result<Option<T>> {
    let raw: Option<String> = row.get(idx)?;
    raw.map(|s| {
        serde_json::from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, Box::new(e))
        })
    })
    .transpose()
}

pub(super) fn id_list<T: Serialize>(ids: &[T]) -> Result<String, StoreError> {
    to_json(ids)
}
