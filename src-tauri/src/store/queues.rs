use std::collections::HashSet;

use rusqlite::{Row, params};
use serde::{Deserialize, Serialize};

use crate::domain::{
    AccountId, ActionNo, Addr, AttachmentId, DraftContentRef, DraftDefaults, DraftId, DraftKind,
    DraftLifecycle, DraftStage, DraftWait, GmailAttachmentId, GmailDraftId, GmailMessageId,
    MessageId, Millis, OpId, OpKind, OpQueue, OutboxId, OutboxState, PinKind, Rev, SplitId,
    ThreadChange, ThreadId, UndoEntryId,
};

use super::sql::{id_list, json_col, json_col_opt, to_json};
use super::{Db, StoreError, Table, UNDO_KEPT, WriteTx};

const DRAFT_COLS: &str = "id, account_id, kind, reply_all, thread_id, target_message_id, started_on_latest, from_addr, to_addrs, cc_addrs, bcc_addrs, subject, body_html, snippet, quote_html, attachments, in_reply_to, references_hdr, updated_at, stage, defaults, waits_for, gmail_draft_id, gmail_message_id, gmail_thread_id, gmail_create_sent, gmail_change_skipped, rev";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftAttachmentItem {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: i64,
    pub path: Option<String>,
    #[serde(default)]
    pub source: Option<AttachmentId>,
    #[serde(default)]
    pub gmail_att_id: Option<GmailAttachmentId>,
    #[serde(default)]
    pub added: Option<Rev>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRecord {
    pub id: DraftId,
    pub account: AccountId,
    pub kind: DraftKind,
    pub reply_all: bool,
    pub thread: Option<ThreadId>,
    pub target: Option<MessageId>,
    pub started_on_latest: bool,
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub body_html: String,
    pub snippet: String,
    pub quote_html: Option<String>,
    pub attachments: Vec<DraftAttachmentItem>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub updated_at: Millis,
    pub lifecycle: DraftLifecycle,
    pub rev: Rev,
    pub in_outbox: bool,
}

impl DraftRecord {
    #[must_use]
    pub fn content_ref(&self) -> DraftContentRef<'_> {
        DraftContentRef {
            from: self.from.as_ref(),
            to: &self.to,
            cc: &self.cc,
            bcc: &self.bcc,
            subject: &self.subject,
            body_html: &self.body_html,
        }
    }

    #[must_use]
    pub fn as_write(&self) -> DraftWrite<'_> {
        DraftWrite {
            id: &self.id,
            account: self.account,
            kind: self.kind,
            reply_all: self.reply_all,
            thread: self.thread,
            target: self.target,
            started_on_latest: self.started_on_latest,
            from: self.from.as_ref(),
            to: &self.to,
            cc: &self.cc,
            bcc: &self.bcc,
            subject: &self.subject,
            body_html: &self.body_html,
            snippet: &self.snippet,
            quote_html: self.quote_html.as_deref(),
            attachments: &self.attachments,
            in_reply_to: self.in_reply_to.as_deref(),
            references: &self.references,
            updated_at: self.updated_at,
            lifecycle: &self.lifecycle,
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let stage_str: String = row.get(19)?;
        let defaults: Option<DraftDefaults> = json_col_opt(row, 20)?;
        let stage = match (stage_str.as_str(), defaults) {
            ("fresh", Some(d)) => DraftStage::Fresh { defaults: d },
            ("kept", None) => DraftStage::Kept,
            _ => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    19,
                    rusqlite::types::Type::Text,
                    Box::new(StoreError::Invalid("corrupt draft stage")),
                ));
            }
        };
        let from_raw: String = row.get(7)?;
        let from = if from_raw.is_empty() || from_raw == "null" {
            None
        } else {
            serde_json::from_str(&from_raw).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?
        };
        Ok(Self {
            id: row.get(0)?,
            account: row.get(1)?,
            kind: row.get(2)?,
            reply_all: row.get::<_, i64>(3)? != 0,
            thread: row.get(4)?,
            target: row.get(5)?,
            started_on_latest: row.get::<_, i64>(6)? != 0,
            from,
            to: json_col(row, 8)?,
            cc: json_col(row, 9)?,
            bcc: json_col(row, 10)?,
            subject: row.get(11)?,
            body_html: row.get(12)?,
            snippet: row.get(13)?,
            quote_html: row.get(14)?,
            attachments: json_col(row, 15)?,
            in_reply_to: row.get(16)?,
            references: json_col(row, 17)?,
            updated_at: row.get(18)?,
            lifecycle: DraftLifecycle {
                stage,
                waits_for: row.get(21)?,
                gmail_draft_id: row.get(22)?,
                gmail_message_id: row.get(23)?,
                gmail_thread_id: row.get(24)?,
                gmail_create_sent: row.get::<_, i64>(25)? != 0,
                gmail_change_skipped: row.get::<_, i64>(26)? != 0,
            },
            rev: row.get(27)?,
            in_outbox: row.get::<_, i64>(28)? != 0,
        })
    }
}

#[derive(Debug, Clone)]
pub struct NewDraft {
    pub id: DraftId,
    pub account: AccountId,
    pub kind: DraftKind,
    pub reply_all: bool,
    pub thread: Option<ThreadId>,
    pub target: Option<MessageId>,
    pub started_on_latest: bool,
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub body_html: String,
    pub snippet: String,
    pub quote_html: Option<String>,
    pub attachments: Vec<DraftAttachmentItem>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub updated_at: Millis,
    pub lifecycle: DraftLifecycle,
}

impl NewDraft {
    #[must_use]
    pub fn as_write(&self) -> DraftWrite<'_> {
        DraftWrite {
            id: &self.id,
            account: self.account,
            kind: self.kind,
            reply_all: self.reply_all,
            thread: self.thread,
            target: self.target,
            started_on_latest: self.started_on_latest,
            from: self.from.as_ref(),
            to: &self.to,
            cc: &self.cc,
            bcc: &self.bcc,
            subject: &self.subject,
            body_html: &self.body_html,
            snippet: &self.snippet,
            quote_html: self.quote_html.as_deref(),
            attachments: &self.attachments,
            in_reply_to: self.in_reply_to.as_deref(),
            references: &self.references,
            updated_at: self.updated_at,
            lifecycle: &self.lifecycle,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DraftWrite<'a> {
    pub id: &'a DraftId,
    pub account: AccountId,
    pub kind: DraftKind,
    pub reply_all: bool,
    pub thread: Option<ThreadId>,
    pub target: Option<MessageId>,
    pub started_on_latest: bool,
    pub from: Option<&'a Addr>,
    pub to: &'a [Addr],
    pub cc: &'a [Addr],
    pub bcc: &'a [Addr],
    pub subject: &'a str,
    pub body_html: &'a str,
    pub snippet: &'a str,
    pub quote_html: Option<&'a str>,
    pub attachments: &'a [DraftAttachmentItem],
    pub in_reply_to: Option<&'a str>,
    pub references: &'a [String],
    pub updated_at: Millis,
    pub lifecycle: &'a DraftLifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxItem {
    pub id: OutboxId,
    pub account: AccountId,
    pub draft: DraftId,
    pub thread: Option<ThreadId>,
    pub send_after: Millis,
    pub state: OutboxState,
    pub gmail_draft_id: Option<GmailDraftId>,
    pub sent_message_id: Option<GmailMessageId>,
    pub undo_requested: bool,
    pub toast_closed: bool,
    pub delayed: bool,
    pub rev: Rev,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRecord {
    pub item: OutboxItem,
    pub to: Vec<Addr>,
    pub snippet: String,
    pub subject: String,
    pub undo_entry: Option<UndoEntryId>,
}

const OUTBOX_JOIN_SELECT: &str = "SELECT o.id, o.account_id, o.draft_id, o.thread_id, o.send_after, o.state, o.gmail_draft_id, o.sent_message_id, o.undo_requested, o.toast_closed, o.delayed, o.rev, COALESCE(d.to_addrs, '[]'), COALESCE(d.snippet, ''), COALESCE(d.subject, ''), (SELECT u.id FROM undo_entries u WHERE u.outbox_id = o.id LIMIT 1) FROM outbox o LEFT JOIN drafts d ON d.id = o.draft_id";

impl OutboxRecord {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            item: OutboxItem {
                id: row.get(0)?,
                account: row.get(1)?,
                draft: row.get(2)?,
                thread: row.get(3)?,
                send_after: row.get(4)?,
                state: row.get(5)?,
                gmail_draft_id: row.get(6)?,
                sent_message_id: row.get(7)?,
                undo_requested: row.get::<_, i64>(8)? != 0,
                toast_closed: row.get::<_, i64>(9)? != 0,
                delayed: row.get::<_, i64>(10)? != 0,
                rev: row.get(11)?,
            },
            to: json_col(row, 12)?,
            snippet: row.get(13)?,
            subject: row.get(14)?,
            undo_entry: row.get(15)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpRecord {
    pub id: OpId,
    pub account: AccountId,
    pub action: Option<ActionNo>,
    pub kind: OpKind,
    pub draft_id: Option<DraftId>,
    pub gmail_draft_id: Option<GmailDraftId>,
    pub payload: String,
}

impl OpRecord {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            account: row.get(1)?,
            action: row.get(2)?,
            kind: row.get(3)?,
            draft_id: row.get(4)?,
            gmail_draft_id: row.get(5)?,
            payload: row.get(6)?,
        })
    }

    pub fn parse_payload<T: serde::de::DeserializeOwned>(&self) -> Result<T, StoreError> {
        serde_json::from_str(&self.payload).map_err(StoreError::Json)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UndoAction {
    Mail {
        threads: Vec<ThreadChange>,
    },
    Send {
        outbox: OutboxId,
    },
    Pin {
        pin_kind: PinKind,
        value: String,
        previous: Option<SplitId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoRecord {
    pub id: UndoEntryId,
    pub account: AccountId,
    pub outbox: Option<OutboxId>,
    pub action: UndoAction,
}

impl UndoRecord {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            account: row.get(1)?,
            outbox: row.get(2)?,
            action: json_col(row, 3)?,
        })
    }
}

impl Db<'_> {
    pub fn draft(self, id: &DraftId) -> Result<Option<DraftRecord>, StoreError> {
        let sql = format!(
            "SELECT {DRAFT_COLS}, EXISTS(SELECT 1 FROM outbox o WHERE o.draft_id = drafts.id) FROM drafts WHERE id = ?1"
        );
        self.optional(&sql, [id], "read draft", DraftRecord::from_row)
    }

    pub fn draft_by_gmail(
        self,
        account: AccountId,
        gmail_id: &GmailDraftId,
    ) -> Result<Option<DraftRecord>, StoreError> {
        let sql = format!(
            "SELECT {DRAFT_COLS}, EXISTS(SELECT 1 FROM outbox o WHERE o.draft_id = drafts.id) FROM drafts WHERE account_id = ?1 AND gmail_draft_id = ?2"
        );
        self.optional(
            &sql,
            params![account, gmail_id],
            "read draft by gmail id",
            DraftRecord::from_row,
        )
    }

    pub fn drafts_for_account(self, account: AccountId) -> Result<Vec<DraftRecord>, StoreError> {
        let sql = format!(
            "SELECT {DRAFT_COLS}, EXISTS(SELECT 1 FROM outbox o WHERE o.draft_id = drafts.id) FROM drafts WHERE account_id = ?1 ORDER BY updated_at DESC, id"
        );
        self.all(
            &sql,
            [account],
            "list account drafts",
            DraftRecord::from_row,
        )
    }

    pub fn drafts_list(self, account: AccountId) -> Result<Vec<DraftRecord>, StoreError> {
        let sql = format!(
            "SELECT {DRAFT_COLS}, 0 FROM drafts WHERE account_id = ?1 AND stage = 'kept' AND NOT EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = drafts.id) ORDER BY updated_at DESC, id"
        );
        self.all(
            &sql,
            [account],
            "list visible drafts",
            DraftRecord::from_row,
        )
    }

    pub fn drafts_by_ids(self, ids: &[DraftId]) -> Result<Vec<DraftRecord>, StoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let encoded = id_list(ids)?;
        let sql = format!(
            "SELECT {DRAFT_COLS}, EXISTS(SELECT 1 FROM outbox o WHERE o.draft_id = drafts.id) FROM drafts WHERE id IN (SELECT value FROM json_each(?1))"
        );
        self.all(&sql, [encoded], "batch read drafts", DraftRecord::from_row)
    }

    pub fn drafts_on_thread(self, thread: ThreadId) -> Result<Vec<DraftRecord>, StoreError> {
        let sql = format!(
            "SELECT {DRAFT_COLS}, 0 FROM drafts WHERE thread_id = ?1 AND NOT EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = drafts.id) ORDER BY updated_at ASC, id"
        );
        self.all(&sql, [thread], "list thread drafts", DraftRecord::from_row)
    }

    pub fn drafts_awaiting_body(self, message: MessageId) -> Result<Vec<DraftRecord>, StoreError> {
        let sql = format!(
            "SELECT {DRAFT_COLS}, EXISTS(SELECT 1 FROM outbox o WHERE o.draft_id = drafts.id) FROM drafts WHERE target_message_id = ?1 AND waits_for = ?2"
        );
        self.all(
            &sql,
            params![message, DraftWait::TargetBody],
            "list drafts awaiting target body",
            DraftRecord::from_row,
        )
    }

    pub fn all_draft_ids(self) -> Result<HashSet<DraftId>, StoreError> {
        let rows = self.all("SELECT id FROM drafts", [], "list all draft ids", |r| {
            r.get::<_, DraftId>(0)
        })?;
        Ok(rows.into_iter().collect())
    }

    pub fn outbox_for_account(self, account: AccountId) -> Result<Vec<OutboxRecord>, StoreError> {
        let sql = format!("{OUTBOX_JOIN_SELECT} WHERE o.account_id = ?1 ORDER BY o.id");
        self.all(
            &sql,
            [account],
            "list account outbox",
            OutboxRecord::from_row,
        )
    }

    pub fn outbox_for_thread(self, thread: ThreadId) -> Result<Vec<OutboxRecord>, StoreError> {
        let sql = format!("{OUTBOX_JOIN_SELECT} WHERE o.thread_id = ?1 ORDER BY o.id");
        self.all(&sql, [thread], "list thread outbox", OutboxRecord::from_row)
    }

    pub fn outbox_unsent(self, account: AccountId) -> Result<Vec<OutboxRecord>, StoreError> {
        let sql = format!(
            "{OUTBOX_JOIN_SELECT} WHERE o.account_id = ?1 AND o.state != ?2 ORDER BY o.send_after, o.id"
        );
        self.all(
            &sql,
            params![account, OutboxState::Sent],
            "list unsent outbox",
            OutboxRecord::from_row,
        )
    }

    pub fn outbox_all(self) -> Result<Vec<OutboxRecord>, StoreError> {
        let sql = format!("{OUTBOX_JOIN_SELECT} ORDER BY o.id");
        self.all(&sql, [], "list all outbox", OutboxRecord::from_row)
    }

    pub fn outbox_row(self, id: OutboxId) -> Result<Option<OutboxRecord>, StoreError> {
        let sql = format!("{OUTBOX_JOIN_SELECT} WHERE o.id = ?1");
        self.optional(&sql, [id], "read outbox row", OutboxRecord::from_row)
    }

    pub fn outbox_gmail_draft_ids(
        self,
        account: AccountId,
    ) -> Result<HashSet<GmailDraftId>, StoreError> {
        let rows = self.all(
            "SELECT gmail_draft_id FROM outbox WHERE account_id = ?1 AND gmail_draft_id IS NOT NULL",
            [account],
            "list outbox gmail draft ids",
            |r| r.get::<_, GmailDraftId>(0),
        )?;
        Ok(rows.into_iter().collect())
    }

    pub fn ops_count(self, account: AccountId) -> Result<u32, StoreError> {
        let count: i64 = self.one(
            "SELECT COUNT(*) FROM ops WHERE account_id = ?1",
            [account],
            "count account ops",
            |r| r.get(0),
        )?;
        Ok(u32::try_from(count.max(0)).unwrap_or(u32::MAX))
    }

    pub fn next_op(
        self,
        account: AccountId,
        queue: OpQueue,
    ) -> Result<Option<OpRecord>, StoreError> {
        let is_draft = i64::from(queue == OpQueue::Drafts);
        self.optional(
            "SELECT id, account_id, action, kind, draft_id, gmail_draft_id, payload FROM ops WHERE account_id = ?1 AND (kind IN (?2, ?3)) = ?4 ORDER BY id LIMIT 1",
            params![account, OpKind::DraftUpsert, OpKind::DraftDelete, is_draft],
            "read next op",
            OpRecord::from_row,
        )
    }

    pub fn all_ops(self, account: AccountId) -> Result<Vec<OpRecord>, StoreError> {
        self.all(
            "SELECT id, account_id, action, kind, draft_id, gmail_draft_id, payload FROM ops WHERE account_id = ?1 ORDER BY id",
            [account],
            "list account ops",
            OpRecord::from_row,
        )
    }

    pub fn has_draft_upsert(self, draft: &DraftId) -> Result<bool, StoreError> {
        let found: Option<i64> = self.optional(
            "SELECT 1 FROM ops WHERE kind = ?1 AND draft_id = ?2 LIMIT 1",
            params![OpKind::DraftUpsert, draft],
            "check draft upsert op",
            |r| r.get(0),
        )?;
        Ok(found.is_some())
    }

    pub fn has_draft_delete_for_gmail(
        self,
        account: AccountId,
        gmail_draft_id: &GmailDraftId,
    ) -> Result<bool, StoreError> {
        let found: Option<i64> = self.optional(
            "SELECT 1 FROM ops WHERE account_id = ?1 AND kind = ?2 AND gmail_draft_id = ?3 LIMIT 1",
            params![account, OpKind::DraftDelete, gmail_draft_id],
            "check draft delete op",
            |r| r.get(0),
        )?;
        Ok(found.is_some())
    }

    pub fn undo_entries(self, account: AccountId) -> Result<Vec<UndoRecord>, StoreError> {
        self.all(
            "SELECT id, account_id, outbox_id, payload FROM undo_entries WHERE account_id = ?1 ORDER BY id",
            [account],
            "list undo entries",
            UndoRecord::from_row,
        )
    }

    pub fn undo_entry(
        self,
        account: AccountId,
        id: UndoEntryId,
    ) -> Result<Option<UndoRecord>, StoreError> {
        self.optional(
            "SELECT id, account_id, outbox_id, payload FROM undo_entries WHERE account_id = ?1 AND id = ?2",
            params![account, id],
            "read undo entry",
            UndoRecord::from_row,
        )
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NewOutbox<'a> {
    pub account: AccountId,
    pub draft: &'a DraftId,
    pub thread: Option<ThreadId>,
    pub send_after: Millis,
    pub gmail_draft_id: Option<&'a GmailDraftId>,
}

#[derive(Debug, Clone, Copy)]
pub struct NewOp<'a, P> {
    pub account: AccountId,
    pub action: Option<ActionNo>,
    pub kind: OpKind,
    pub draft_id: Option<&'a DraftId>,
    pub gmail_draft_id: Option<&'a GmailDraftId>,
    pub payload: &'a P,
}

impl WriteTx<'_> {
    pub fn put_draft(&mut self, draft: &DraftWrite<'_>) -> Result<Rev, StoreError> {
        let old_thread = self.read().draft(draft.id)?.and_then(|d| d.thread);
        let rev = self.stamp();
        let (stage_str, defaults_json) = match &draft.lifecycle.stage {
            DraftStage::Fresh { defaults } => ("fresh", Some(to_json(defaults)?)),
            DraftStage::Kept => ("kept", None),
        };
        let from_json = match draft.from {
            Some(a) => to_json(a)?,
            None => String::new(),
        };
        self.read().execute(
            "INSERT INTO drafts (id, account_id, kind, reply_all, thread_id, target_message_id, started_on_latest, from_addr, to_addrs, cc_addrs, bcc_addrs, subject, body_html, snippet, quote_html, attachments, in_reply_to, references_hdr, updated_at, stage, defaults, waits_for, gmail_draft_id, gmail_message_id, gmail_thread_id, gmail_create_sent, gmail_change_skipped, rev)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28)
             ON CONFLICT(id) DO UPDATE SET
               kind = excluded.kind, reply_all = excluded.reply_all, thread_id = excluded.thread_id,
               target_message_id = excluded.target_message_id, started_on_latest = excluded.started_on_latest,
               from_addr = excluded.from_addr, to_addrs = excluded.to_addrs, cc_addrs = excluded.cc_addrs,
               bcc_addrs = excluded.bcc_addrs, subject = excluded.subject, body_html = excluded.body_html,
               snippet = excluded.snippet, quote_html = excluded.quote_html, attachments = excluded.attachments,
               in_reply_to = excluded.in_reply_to, references_hdr = excluded.references_hdr,
               updated_at = excluded.updated_at, stage = excluded.stage, defaults = excluded.defaults,
               waits_for = excluded.waits_for, gmail_draft_id = excluded.gmail_draft_id,
               gmail_message_id = excluded.gmail_message_id, gmail_thread_id = excluded.gmail_thread_id,
               gmail_create_sent = excluded.gmail_create_sent, gmail_change_skipped = excluded.gmail_change_skipped,
               rev = excluded.rev",
            params![
                draft.id,
                draft.account,
                draft.kind,
                i64::from(draft.reply_all),
                draft.thread,
                draft.target,
                i64::from(draft.started_on_latest),
                from_json,
                to_json(&draft.to)?,
                to_json(&draft.cc)?,
                to_json(&draft.bcc)?,
                draft.subject,
                draft.body_html,
                draft.snippet,
                draft.quote_html,
                to_json(&draft.attachments)?,
                draft.in_reply_to,
                to_json(&draft.references)?,
                draft.updated_at,
                stage_str,
                defaults_json,
                draft.lifecycle.waits_for,
                draft.lifecycle.gmail_draft_id,
                draft.lifecycle.gmail_message_id,
                draft.lifecycle.gmail_thread_id,
                i64::from(draft.lifecycle.gmail_create_sent),
                i64::from(draft.lifecycle.gmail_change_skipped),
                rev,
            ],
            "upsert draft",
        )?;
        self.record().draft(draft.account, draft.id.clone());
        self.stamp_threads(draft.account, [old_thread, draft.thread], rev)?;
        Ok(rev)
    }

    pub fn delete_draft(&mut self, id: &DraftId) -> Result<Option<DraftRecord>, StoreError> {
        let Some(existing) = self.read().draft(id)? else {
            return Ok(None);
        };
        let rev = self.stamp();
        self.read()
            .execute("DELETE FROM drafts WHERE id = ?1", [id], "delete draft")?;
        self.record().draft(existing.account, existing.id.clone());
        self.stamp_threads(existing.account, [existing.thread], rev)?;
        Ok(Some(existing))
    }

    pub fn insert_outbox(&mut self, item: &NewOutbox<'_>) -> Result<OutboxId, StoreError> {
        let rev = self.stamp();
        let id = OutboxId(self.read().insert(
            "INSERT INTO outbox (account_id, draft_id, thread_id, send_after, state, gmail_draft_id, rev) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                item.account,
                item.draft,
                item.thread,
                item.send_after,
                OutboxState::Waiting,
                item.gmail_draft_id,
                rev,
            ],
            "insert outbox row",
        )?);
        self.touch(Some(item.account), Table::Outbox);
        self.record().draft(item.account, item.draft.clone());
        self.stamp_threads(item.account, [item.thread], rev)?;
        Ok(id)
    }

    pub fn update_outbox(&mut self, item: &OutboxItem) -> Result<(), StoreError> {
        let old_thread = self.read().outbox_row(item.id)?.and_then(|r| r.item.thread);
        let rev = self.stamp();
        self.read().execute(
            "UPDATE outbox SET thread_id = ?2, send_after = ?3, state = ?4, gmail_draft_id = ?5, sent_message_id = ?6, undo_requested = ?7, toast_closed = ?8, delayed = ?9, rev = ?10 WHERE id = ?1",
            params![
                item.id,
                item.thread,
                item.send_after,
                item.state,
                item.gmail_draft_id,
                item.sent_message_id,
                i64::from(item.undo_requested),
                i64::from(item.toast_closed),
                i64::from(item.delayed),
                rev,
            ],
            "update outbox row",
        )?;
        self.touch(Some(item.account), Table::Outbox);
        self.stamp_threads(item.account, [old_thread, item.thread], rev)?;
        Ok(())
    }

    pub fn delete_outbox(&mut self, id: OutboxId) -> Result<Option<OutboxRecord>, StoreError> {
        let Some(existing) = self.read().outbox_row(id)? else {
            return Ok(None);
        };
        let rev = self.stamp();
        self.read().execute(
            "DELETE FROM outbox WHERE id = ?1",
            [id],
            "delete outbox row",
        )?;
        self.touch(Some(existing.item.account), Table::Outbox);
        self.record()
            .draft(existing.item.account, existing.item.draft.clone());
        self.stamp_threads(existing.item.account, [existing.item.thread], rev)?;
        Ok(Some(existing))
    }

    pub fn queue_op<P: Serialize>(&mut self, op: &NewOp<'_, P>) -> Result<OpId, StoreError> {
        let encoded = to_json(op.payload)?;
        let id = OpId(self.read().insert(
            "INSERT INTO ops (account_id, action, kind, draft_id, gmail_draft_id, payload) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                op.account,
                op.action,
                op.kind,
                op.draft_id,
                op.gmail_draft_id,
                encoded,
            ],
            "queue op",
        )?);
        self.touch(Some(op.account), Table::Ops);
        Ok(id)
    }

    pub fn update_op_payload<P: Serialize>(
        &mut self,
        id: OpId,
        account: AccountId,
        payload: &P,
    ) -> Result<(), StoreError> {
        let encoded = to_json(payload)?;
        let changed = self.read().execute(
            "UPDATE ops SET payload = ?2 WHERE id = ?1",
            params![id, encoded],
            "update op payload",
        )?;
        if changed > 0 {
            self.touch(Some(account), Table::Ops);
        }
        Ok(())
    }

    pub fn delete_op(&mut self, id: OpId, account: AccountId) -> Result<(), StoreError> {
        let changed = self
            .read()
            .execute("DELETE FROM ops WHERE id = ?1", [id], "delete op")?;
        if changed > 0 {
            self.touch(Some(account), Table::Ops);
        }
        Ok(())
    }

    pub fn delete_draft_upserts(
        &mut self,
        account: AccountId,
        draft: &DraftId,
    ) -> Result<(), StoreError> {
        let changed = self.read().execute(
            "DELETE FROM ops WHERE account_id = ?1 AND kind = ?2 AND draft_id = ?3",
            params![account, OpKind::DraftUpsert, draft],
            "delete draft upsert ops",
        )?;
        if changed > 0 {
            self.touch(Some(account), Table::Ops);
        }
        Ok(())
    }

    pub fn record_undo(
        &mut self,
        id: UndoEntryId,
        account: AccountId,
        outbox: Option<OutboxId>,
        action: &UndoAction,
    ) -> Result<(), StoreError> {
        let encoded = to_json(action)?;
        let db = self.read();
        db.execute(
            "INSERT OR REPLACE INTO undo_entries (id, account_id, outbox_id, payload) VALUES (?1, ?2, ?3, ?4)",
            params![id, account, outbox, encoded],
            "insert undo entry",
        )?;
        let keep = i64::try_from(UNDO_KEPT).unwrap_or(50);
        db.execute(
            "DELETE FROM undo_entries WHERE account_id = ?1 AND id NOT IN (SELECT id FROM undo_entries WHERE account_id = ?1 ORDER BY id DESC LIMIT ?2)",
            params![account, keep],
            "trim undo entries",
        )?;
        self.touch(Some(account), Table::Undo);
        Ok(())
    }

    pub fn delete_undo(&mut self, account: AccountId, id: UndoEntryId) -> Result<(), StoreError> {
        let changed = self.read().execute(
            "DELETE FROM undo_entries WHERE account_id = ?1 AND id = ?2",
            params![account, id],
            "delete undo entry",
        )?;
        if changed > 0 {
            self.touch(Some(account), Table::Undo);
        }
        Ok(())
    }

    pub fn delete_undo_for_outbox(
        &mut self,
        account: AccountId,
        outbox: OutboxId,
    ) -> Result<(), StoreError> {
        let changed = self.read().execute(
            "DELETE FROM undo_entries WHERE account_id = ?1 AND outbox_id = ?2",
            params![account, outbox],
            "delete undo for outbox",
        )?;
        if changed > 0 {
            self.touch(Some(account), Table::Undo);
        }
        Ok(())
    }

    pub fn update_undo_payload(
        &mut self,
        account: AccountId,
        id: UndoEntryId,
        action: &UndoAction,
    ) -> Result<(), StoreError> {
        let encoded = to_json(action)?;
        let changed = self.read().execute(
            "UPDATE undo_entries SET payload = ?3 WHERE account_id = ?1 AND id = ?2",
            params![account, id, encoded],
            "update undo payload",
        )?;
        if changed > 0 {
            self.touch(Some(account), Table::Undo);
        }
        Ok(())
    }

    fn stamp_threads<const N: usize>(
        &mut self,
        account: AccountId,
        threads: [Option<ThreadId>; N],
        rev: Rev,
    ) -> Result<(), StoreError> {
        let mut seen = [None; N];
        let mut count = 0;
        for tid in threads.into_iter().flatten() {
            if seen[..count].contains(&Some(tid)) {
                continue;
            }
            seen[count] = Some(tid);
            count += 1;
            self.read().execute(
                "UPDATE threads SET rev = ?2 WHERE id = ?1",
                params![tid, rev],
                "stamp thread rev",
            )?;
            self.record().thread(account, tid);
        }
        Ok(())
    }
}
