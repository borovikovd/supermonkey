//! The Gmail op queue (SPEC Offline): mutations waiting to run, replayed in
//! id order per account. Payloads are the caller's JSON; the store keeps the
//! columns it must query by.

use std::fmt;
use std::str::FromStr;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{Row, named_params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sm_domain::{AccountId, ActionNo, DraftId, GmailDraftId, Millis, UnknownKind};

use crate::db::{Db, to_json};
use crate::tx::WriteTx;
use crate::{StoreError, Table};

/// A queued op's id. Never reused: an op deleted while its runner works on
/// it must not lend its id to a new op the runner would then delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OpId(pub i64);

impl fmt::Display for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl ToSql for OpId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        self.0.to_sql()
    }
}

impl FromSql for OpId {
    fn column_result(v: ValueRef<'_>) -> FromSqlResult<Self> {
        i64::column_result(v).map(Self)
    }
}

/// What a queued op does at Gmail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpKind {
    /// `messages.batchModify`: labels added and removed.
    Modify,
    /// `labels.create`.
    LabelCreate,
    /// `drafts.create` or `drafts.update`.
    DraftUpsert,
    /// `drafts.delete`.
    DraftDelete,
    /// `messages.trash`, for undoing a send already sent.
    TrashMessage,
}

impl OpKind {
    /// The stored text.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Modify => "modify",
            Self::LabelCreate => "label_create",
            Self::DraftUpsert => "draft_upsert",
            Self::DraftDelete => "draft_delete",
            Self::TrashMessage => "trash_message",
        }
    }
}

impl FromStr for OpKind {
    type Err = UnknownKind;

    fn from_str(s: &str) -> Result<Self, UnknownKind> {
        [
            Self::Modify,
            Self::LabelCreate,
            Self::DraftUpsert,
            Self::DraftDelete,
            Self::TrashMessage,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
        .ok_or_else(|| UnknownKind {
            kind: "OpKind",
            value: s.to_owned(),
        })
    }
}

impl ToSql for OpKind {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

impl FromSql for OpKind {
    fn column_result(v: ValueRef<'_>) -> FromSqlResult<Self> {
        v.as_str()?
            .parse()
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

/// An op to queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOp<'a, P> {
    /// Its account.
    pub account: AccountId,
    /// The UI action that queued it, if any.
    pub action: Option<ActionNo>,
    /// What it does.
    pub kind: OpKind,
    /// The local draft it concerns, so a send or a newer save can drop it.
    pub draft: Option<&'a DraftId>,
    /// The Gmail draft it concerns.
    pub gmail_draft: Option<&'a GmailDraftId>,
    /// The caller's payload, stored as JSON.
    pub payload: &'a P,
    /// When it was queued.
    pub created_at: Millis,
}

/// A queued op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpRecord {
    /// Its id.
    pub id: OpId,
    /// Its account.
    pub account: AccountId,
    /// The UI action that queued it.
    pub action: Option<ActionNo>,
    /// What it does.
    pub kind: OpKind,
    /// The local draft it concerns.
    pub draft: Option<DraftId>,
    /// The Gmail draft it concerns.
    pub gmail_draft: Option<GmailDraftId>,
    /// The payload's JSON; [`OpRecord::payload`] decodes it.
    pub payload_json: String,
    /// When it was queued.
    pub created_at: Millis,
}

impl OpRecord {
    /// The payload as the type it was queued with.
    pub fn payload<P: DeserializeOwned>(&self) -> Result<P, serde_json::Error> {
        serde_json::from_str(&self.payload_json)
    }
}

fn op_row(r: &Row<'_>) -> rusqlite::Result<OpRecord> {
    Ok(OpRecord {
        id: r.get("id")?,
        account: r.get("account_id")?,
        action: r.get("action")?,
        kind: r.get("kind")?,
        draft: r.get("draft_id")?,
        gmail_draft: r.get("gmail_draft_id")?,
        payload_json: r.get("payload")?,
        created_at: r.get("created_at")?,
    })
}

/// Reads of the op queue.
#[derive(Clone, Copy)]
pub struct OpReads<'c> {
    db: Db<'c>,
}

impl<'c> OpReads<'c> {
    pub(crate) fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// The account's next op to run.
    pub fn next(self, account: AccountId) -> Result<Option<OpRecord>, StoreError> {
        self.db.optional(
            "read the next op",
            "SELECT id, account_id, action, kind, draft_id, gmail_draft_id, payload,
                    created_at
             FROM ops WHERE account_id = ?1 ORDER BY id LIMIT 1",
            [account],
            op_row,
        )
    }

    /// One op.
    pub fn get(self, id: OpId) -> Result<Option<OpRecord>, StoreError> {
        self.db.optional(
            "read an op",
            "SELECT id, account_id, action, kind, draft_id, gmail_draft_id, payload,
                    created_at
             FROM ops WHERE id = ?1",
            [id],
            op_row,
        )
    }

    /// The account's queued ops of one kind, in run order.
    pub fn of_kind(self, account: AccountId, kind: OpKind) -> Result<Vec<OpRecord>, StoreError> {
        self.db.all(
            "read queued ops",
            "SELECT id, account_id, action, kind, draft_id, gmail_draft_id, payload,
                    created_at
             FROM ops WHERE account_id = ?1 AND kind = ?2 ORDER BY id",
            (account, kind),
            op_row,
        )
    }

    /// How many ops the account has queued.
    pub fn count(self, account: AccountId) -> Result<u32, StoreError> {
        self.db.one(
            "count queued ops",
            "SELECT count(*) AS n FROM ops WHERE account_id = ?1",
            [account],
            |r| r.get("n"),
        )
    }

    /// Whether a delete of this Gmail draft is queued.
    pub fn deletes_gmail_draft(
        self,
        account: AccountId,
        gmail_draft: &GmailDraftId,
    ) -> Result<bool, StoreError> {
        self.db.one(
            "check for a queued draft delete",
            "SELECT EXISTS (SELECT 1 FROM ops
                            WHERE account_id = ?1 AND kind = 'draft_delete'
                              AND gmail_draft_id = ?2) AS queued",
            (account, gmail_draft),
            |r| r.get("queued"),
        )
    }

    /// Whether a delete of every Gmail copy of this local draft is queued: it
    /// was discarded while a `drafts.create` of it had no answer, so Gmail
    /// may hold a copy the app has no id for.
    pub fn deletes_copies_of(
        self,
        account: AccountId,
        draft: &DraftId,
    ) -> Result<bool, StoreError> {
        self.db.one(
            "check for a queued delete of a draft's copies",
            "SELECT EXISTS (SELECT 1 FROM ops
                            WHERE account_id = ?1 AND kind = 'draft_delete'
                              AND draft_id = ?2 AND gmail_draft_id IS NULL) AS queued",
            (account, draft),
            |r| r.get("queued"),
        )
    }

    /// Whether an upsert of the local draft is queued: it holds an edit Gmail
    /// has not confirmed, waiting to run or to retry.
    pub fn upserts_draft(self, draft: &DraftId) -> Result<bool, StoreError> {
        self.db.one(
            "check for a queued draft upsert",
            "SELECT EXISTS (SELECT 1 FROM ops
                            WHERE kind = 'draft_upsert' AND draft_id = ?1) AS queued",
            [draft],
            |r| r.get("queued"),
        )
    }

    /// The account's local drafts with a queued upsert (see
    /// [`Self::upserts_draft`]).
    pub fn upserted_drafts(self, account: AccountId) -> Result<Vec<DraftId>, StoreError> {
        self.db.all(
            "read the drafts with a queued upsert",
            "SELECT DISTINCT draft_id FROM ops
             WHERE account_id = ?1 AND kind = 'draft_upsert' AND draft_id IS NOT NULL
             ORDER BY draft_id",
            [account],
            |r| r.get("draft_id"),
        )
    }
}

/// Writes to the op queue.
pub struct OpWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> OpWrites<'t, 'c> {
    pub(crate) fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Queues an op last. Returns its id.
    pub fn push<P: Serialize>(&mut self, op: &NewOp<'_, P>) -> Result<OpId, StoreError> {
        let id = OpId(self.tx.db().insert(
            "queue an op",
            "INSERT INTO ops (account_id, action, kind, draft_id, gmail_draft_id, payload,
                 created_at)
             VALUES (:account, :action, :kind, :draft, :gmail_draft, :payload, :created_at)",
            named_params! {
                ":account": op.account, ":action": op.action, ":kind": op.kind,
                ":draft": op.draft, ":gmail_draft": op.gmail_draft,
                ":payload": to_json("payload", op.payload)?, ":created_at": op.created_at,
            },
        )?);
        self.tx.touch(op.account, Table::Ops);
        Ok(id)
    }

    /// Replaces an op's payload, as a runner drops ids Gmail no longer has.
    pub fn set_payload<P: Serialize>(&mut self, id: OpId, payload: &P) -> Result<(), StoreError> {
        let account: Option<AccountId> = self.tx.db().optional(
            "store an op's payload",
            "UPDATE ops SET payload = ?2 WHERE id = ?1 RETURNING account_id",
            (id, to_json("payload", payload)?),
            |r| r.get("account_id"),
        )?;
        if let Some(account) = account {
            self.tx.touch(account, Table::Ops);
        }
        Ok(())
    }

    /// Removes an op: done, rejected, or dropped. Unknown ids are skipped.
    pub fn delete(&mut self, id: OpId) -> Result<(), StoreError> {
        let account: Option<AccountId> = self.tx.db().optional(
            "remove an op",
            "DELETE FROM ops WHERE id = ?1 RETURNING account_id",
            [id],
            |r| r.get("account_id"),
        )?;
        if let Some(account) = account {
            self.tx.touch(account, Table::Ops);
        }
        Ok(())
    }

    /// Drops a draft's queued upserts: a send or discard writes the final
    /// version itself, and a newer save queues its own.
    pub fn delete_draft_upserts(&mut self, draft: &DraftId) -> Result<(), StoreError> {
        let accounts: Vec<AccountId> = self.tx.db().all(
            "drop a draft's queued upserts",
            "DELETE FROM ops WHERE kind = 'draft_upsert' AND draft_id = ?1 RETURNING account_id",
            [draft],
            |r| r.get("account_id"),
        )?;
        for account in accounts {
            self.tx.touch(account, Table::Ops);
        }
        Ok(())
    }
}
