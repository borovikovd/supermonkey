use rusqlite::{Row, named_params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sm_domain::{AccountId, ActionNo, DraftId, GmailDraftId, OpId, OpKind, OpQueue};

use crate::db::{Db, to_json};
use crate::tx::WriteTx;
use crate::{StoreError, Table};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOp<'a, P> {
    pub account: AccountId,
    pub action: Option<ActionNo>,
    pub kind: OpKind,
    pub draft: Option<&'a DraftId>,
    pub gmail_draft: Option<&'a GmailDraftId>,
    pub payload: &'a P,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpRecord {
    pub id: OpId,
    pub account: AccountId,
    pub action: Option<ActionNo>,
    pub kind: OpKind,
    pub draft: Option<DraftId>,
    pub gmail_draft: Option<GmailDraftId>,
    pub payload_json: String,
}

impl OpRecord {
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
    })
}

#[derive(Debug, Clone, Copy)]
pub struct OpReads<'c> {
    db: Db<'c>,
}

impl<'c> OpReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    pub fn next(self, account: AccountId, queue: OpQueue) -> Result<Option<OpRecord>, StoreError> {
        self.db.optional(
            "read the next op",
            "SELECT id, account_id, action, kind, draft_id, gmail_draft_id, payload
             FROM ops WHERE account_id = ?1 AND (kind IN (?2, ?3)) = ?4 ORDER BY id LIMIT 1",
            (
                account,
                OpKind::DraftUpsert,
                OpKind::DraftDelete,
                queue == OpQueue::Drafts,
            ),
            op_row,
        )
    }

    pub fn get(self, id: OpId) -> Result<Option<OpRecord>, StoreError> {
        self.db.optional(
            "read an op",
            "SELECT id, account_id, action, kind, draft_id, gmail_draft_id, payload
             FROM ops WHERE id = ?1",
            [id],
            op_row,
        )
    }

    pub fn of_kind(self, account: AccountId, kind: OpKind) -> Result<Vec<OpRecord>, StoreError> {
        self.db.all(
            "read queued ops",
            "SELECT id, account_id, action, kind, draft_id, gmail_draft_id, payload
             FROM ops WHERE account_id = ?1 AND kind = ?2 ORDER BY id",
            (account, kind),
            op_row,
        )
    }

    pub fn count(self, account: AccountId) -> Result<u32, StoreError> {
        self.db.one(
            "count queued ops",
            "SELECT count(*) AS n FROM ops WHERE account_id = ?1",
            [account],
            |r| r.get("n"),
        )
    }

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

    pub fn upserts_draft(self, draft: &DraftId) -> Result<bool, StoreError> {
        self.db.one(
            "check for a queued draft upsert",
            "SELECT EXISTS (SELECT 1 FROM ops
                            WHERE kind = 'draft_upsert' AND draft_id = ?1) AS queued",
            [draft],
            |r| r.get("queued"),
        )
    }

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

#[derive(Debug)]
pub struct OpWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> OpWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    pub fn push<P: Serialize>(&mut self, op: &NewOp<'_, P>) -> Result<OpId, StoreError> {
        let id = OpId(self.tx.db().insert(
            "queue an op",
            "INSERT INTO ops (account_id, action, kind, draft_id, gmail_draft_id, payload)
             VALUES (:account, :action, :kind, :draft, :gmail_draft, :payload)",
            named_params! {
                ":account": op.account, ":action": op.action, ":kind": op.kind,
                ":draft": op.draft, ":gmail_draft": op.gmail_draft,
                ":payload": to_json("payload", op.payload)?,
            },
        )?);
        self.tx.touch(op.account, Table::Ops);
        Ok(id)
    }

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

    pub fn requeue(&mut self, id: OpId) -> Result<(), StoreError> {
        self.tx.db().execute(
            "queue an op again",
            "INSERT INTO ops (account_id, action, kind, draft_id, gmail_draft_id, payload)
             SELECT account_id, action, kind, draft_id, gmail_draft_id, payload
             FROM ops WHERE id = ?1",
            [id],
        )?;
        self.delete(id)
    }

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
