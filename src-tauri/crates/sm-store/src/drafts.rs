use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{Row, named_params};
use serde::{Deserialize, Serialize};
use sm_domain::{
    AccountId, Addr, AttachmentId, DraftContent, DraftContentRef, DraftId, DraftKind, GmailDraftId,
    GmailMessageId, GmailThreadId, MessageId, Millis, Rev, ThreadId,
};

use crate::StoreError;
use crate::db::{Db, id_list, json_column, json_column_opt, to_json};
use crate::tx::WriteTx;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftAttachment {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: i64,
    pub path: Option<String>,
    pub source: Option<AttachmentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<Rev>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftWait {
    Parts,
    TargetBody,
}

impl DraftWait {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Parts => "parts",
            Self::TargetBody => "target_body",
        }
    }
}

impl ToSql for DraftWait {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

impl FromSql for DraftWait {
    fn column_result(v: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = v.as_str()?;
        [Self::Parts, Self::TargetBody]
            .into_iter()
            .find(|w| w.as_str() == text)
            .ok_or(FromSqlError::InvalidType)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DraftStage {
    Fresh {
        defaults: DraftContent,
    },
    #[default]
    Kept,
}

impl DraftStage {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Fresh { .. } => "fresh",
            Self::Kept => "kept",
        }
    }

    const fn defaults(&self) -> Option<&DraftContent> {
        match self {
            Self::Fresh { defaults } => Some(defaults),
            Self::Kept => None,
        }
    }

    fn from_columns(stage: &str, defaults: Option<DraftContent>) -> rusqlite::Result<Self> {
        match (stage, defaults) {
            ("fresh", Some(defaults)) => Ok(Self::Fresh { defaults }),
            ("kept", None) => Ok(Self::Kept),
            (stage, defaults) => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!(
                    "draft stage {stage} {} defaults",
                    if defaults.is_some() {
                        "with"
                    } else {
                        "without"
                    }
                )
                .into(),
            )),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftLifecycle {
    pub stage: DraftStage,
    pub waits_for: Option<DraftWait>,
    pub gmail_draft_id: Option<GmailDraftId>,
    pub gmail_message_id: Option<GmailMessageId>,
    pub gmail_thread_id: Option<GmailThreadId>,
    pub create_sent: bool,
    pub gmail_change_skipped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
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
    pub attachments: Vec<DraftAttachment>,
    pub in_reply_to: Option<String>,
    pub references: String,
    pub updated_at: Millis,
    pub lifecycle: DraftLifecycle,
}

impl Draft {
    #[must_use]
    pub fn new(id: DraftId, account: AccountId, kind: DraftKind, now: Millis) -> Self {
        Self {
            id,
            account,
            kind,
            reply_all: false,
            thread: None,
            target: None,
            started_on_latest: false,
            from: None,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body_html: String::new(),
            snippet: String::new(),
            quote_html: None,
            attachments: Vec::new(),
            in_reply_to: None,
            references: String::new(),
            updated_at: now,
            lifecycle: DraftLifecycle::default(),
        }
    }

    #[must_use]
    pub fn content(&self) -> DraftContentRef<'_, impl Iterator<Item = &str>> {
        DraftContentRef {
            from: self.from.as_ref(),
            to: &self.to,
            cc: &self.cc,
            bcc: &self.bcc,
            subject: &self.subject,
            body_html: &self.body_html,
            attachments: self.attachments.iter().map(|a| a.id.as_str()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRecord {
    pub draft: Draft,
    pub rev: Rev,
    pub in_outbox: bool,
}

fn draft_row(r: &Row<'_>) -> rusqlite::Result<DraftRecord> {
    let stage = r.get_ref("stage")?.as_str()?;
    let stage = DraftStage::from_columns(stage, json_column_opt(r, "defaults")?)?;
    Ok(DraftRecord {
        draft: Draft {
            id: r.get("id")?,
            account: r.get("account_id")?,
            kind: r.get("kind")?,
            reply_all: r.get("reply_all")?,
            thread: r.get("thread_id")?,
            target: r.get("target_message_id")?,
            started_on_latest: r.get("started_on_latest")?,
            from: json_column(r, "from_addr")?,
            to: json_column(r, "to_addrs")?,
            cc: json_column(r, "cc_addrs")?,
            bcc: json_column(r, "bcc_addrs")?,
            subject: r.get("subject")?,
            body_html: r.get("body_html")?,
            snippet: r.get("snippet")?,
            quote_html: r.get("quote_html")?,
            attachments: json_column(r, "attachments")?,
            in_reply_to: r.get("in_reply_to")?,
            references: r.get("references_hdr")?,
            updated_at: r.get("updated_at")?,
            lifecycle: DraftLifecycle {
                stage,
                waits_for: r.get("waits_for")?,
                gmail_draft_id: r.get("gmail_draft_id")?,
                gmail_message_id: r.get("gmail_message_id")?,
                gmail_thread_id: r.get("gmail_thread_id")?,
                create_sent: r.get("gmail_create_sent")?,
                gmail_change_skipped: r.get("gmail_change_skipped")?,
            },
        },
        rev: r.get("rev")?,
        in_outbox: r.get("in_outbox")?,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct DraftReads<'c> {
    db: Db<'c>,
}

impl<'c> DraftReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    pub fn get(self, id: &DraftId) -> Result<Option<DraftRecord>, StoreError> {
        self.db.optional(
            "read a draft",
            "SELECT d.*, EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = d.id) AS in_outbox
             FROM drafts d WHERE d.id = ?1",
            [id],
            draft_row,
        )
    }

    pub fn get_many(self, ids: &[DraftId]) -> Result<Vec<DraftRecord>, StoreError> {
        self.db.all(
            "read drafts",
            "SELECT d.*, EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = d.id) AS in_outbox
             FROM json_each(?1) j JOIN drafts d ON d.id = j.value ORDER BY j.key",
            [id_list(ids)?],
            draft_row,
        )
    }

    pub fn ids(self) -> Result<Vec<DraftId>, StoreError> {
        self.db
            .all("read the drafts' ids", "SELECT id FROM drafts", [], |r| {
                r.get("id")
            })
    }

    pub fn list(self, account: AccountId) -> Result<Vec<DraftId>, StoreError> {
        self.db.all(
            "list Drafts",
            "SELECT d.id FROM drafts d
             WHERE d.account_id = ?1
               AND NOT EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = d.id)
             ORDER BY d.updated_at DESC, d.id",
            [account],
            |r| r.get("id"),
        )
    }

    pub fn on_thread(self, thread: ThreadId) -> Result<Vec<DraftRecord>, StoreError> {
        self.db.all(
            "read a thread's drafts",
            "SELECT d.*, 0 AS in_outbox FROM drafts d
             WHERE d.thread_id = ?1 AND d.kind != 'new'
               AND NOT EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = d.id)
             ORDER BY d.updated_at, d.id",
            [thread],
            draft_row,
        )
    }

    pub fn for_account(self, account: AccountId) -> Result<Vec<DraftRecord>, StoreError> {
        self.db.all(
            "read an account's drafts",
            "SELECT d.*, EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = d.id) AS in_outbox
             FROM drafts d WHERE d.account_id = ?1 ORDER BY d.id",
            [account],
            draft_row,
        )
    }

    pub fn by_gmail_draft(
        self,
        account: AccountId,
        gmail_draft_id: &GmailDraftId,
    ) -> Result<Option<DraftId>, StoreError> {
        self.db.optional(
            "find a draft by Gmail id",
            "SELECT id FROM drafts WHERE account_id = ?1 AND gmail_draft_id = ?2
             ORDER BY id LIMIT 1",
            (account, gmail_draft_id),
            |r| r.get("id"),
        )
    }

    pub fn fresh(self) -> Result<Vec<DraftId>, StoreError> {
        self.db.all(
            "read the fresh drafts",
            "SELECT id FROM drafts WHERE stage = 'fresh' ORDER BY id",
            [],
            |r| r.get("id"),
        )
    }

    pub fn awaiting_body(self, message: MessageId) -> Result<Vec<DraftId>, StoreError> {
        self.db.all(
            "read the drafts waiting for a message's body",
            "SELECT id FROM drafts WHERE target_message_id = ?1 AND waits_for = ?2 ORDER BY id",
            (message, DraftWait::TargetBody),
            |r| r.get("id"),
        )
    }
}

const PUT_DRAFT: &str =
    "INSERT INTO drafts (id, account_id, kind, reply_all, thread_id, target_message_id,
         started_on_latest, from_addr, to_addrs, cc_addrs, bcc_addrs, subject,
         body_html, snippet, quote_html, attachments, in_reply_to, references_hdr,
         updated_at, stage, defaults, waits_for, gmail_draft_id, gmail_message_id,
         gmail_thread_id, gmail_create_sent, gmail_change_skipped, rev)
     VALUES (:id, :account, :kind, :reply_all, :thread, :target, :on_latest, :from,
         :to, :cc, :bcc, :subject, :body, :snippet, :quote, :attachments,
         :in_reply_to, :references, :updated_at, :stage, :defaults, :waits_for,
         :gmail_draft, :gmail_message, :gmail_thread, :create_sent, :change_skipped,
         :rev)
     ON CONFLICT (id) DO UPDATE SET
         kind = excluded.kind, reply_all = excluded.reply_all,
         thread_id = excluded.thread_id, target_message_id = excluded.target_message_id,
         started_on_latest = excluded.started_on_latest, from_addr = excluded.from_addr,
         to_addrs = excluded.to_addrs, cc_addrs = excluded.cc_addrs,
         bcc_addrs = excluded.bcc_addrs, subject = excluded.subject,
         body_html = excluded.body_html, snippet = excluded.snippet,
         quote_html = excluded.quote_html, attachments = excluded.attachments,
         in_reply_to = excluded.in_reply_to, references_hdr = excluded.references_hdr,
         updated_at = excluded.updated_at, stage = excluded.stage,
         defaults = excluded.defaults, waits_for = excluded.waits_for,
         gmail_draft_id = excluded.gmail_draft_id,
         gmail_message_id = excluded.gmail_message_id,
         gmail_thread_id = excluded.gmail_thread_id,
         gmail_create_sent = excluded.gmail_create_sent,
         gmail_change_skipped = excluded.gmail_change_skipped,
         rev = excluded.rev";

#[derive(Debug)]
pub struct DraftWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> DraftWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    pub fn put(&mut self, draft: &Draft) -> Result<(), StoreError> {
        let before = self.placed(&draft.id)?;
        if let Some((account, _)) = before {
            ensure_same_account(&draft.id, account, draft.account)?;
        }
        let rev = self.tx.stamp();
        let life = &draft.lifecycle;
        let defaults = life
            .stage
            .defaults()
            .map(|d| to_json("defaults", d))
            .transpose()?;
        self.tx.db().execute(
            "store a draft",
            PUT_DRAFT,
            named_params! {
                ":id": draft.id,
                ":account": draft.account,
                ":kind": draft.kind,
                ":reply_all": draft.reply_all,
                ":thread": draft.thread,
                ":target": draft.target,
                ":on_latest": draft.started_on_latest,
                ":from": to_json("from_addr", &draft.from)?,
                ":to": to_json("to_addrs", &draft.to)?,
                ":cc": to_json("cc_addrs", &draft.cc)?,
                ":bcc": to_json("bcc_addrs", &draft.bcc)?,
                ":subject": draft.subject,
                ":body": draft.body_html,
                ":snippet": draft.snippet,
                ":quote": draft.quote_html,
                ":attachments": to_json("attachments", &draft.attachments)?,
                ":in_reply_to": draft.in_reply_to,
                ":references": draft.references,
                ":updated_at": draft.updated_at,
                ":stage": life.stage.as_str(),
                ":defaults": defaults,
                ":waits_for": life.waits_for,
                ":gmail_draft": life.gmail_draft_id,
                ":gmail_message": life.gmail_message_id,
                ":gmail_thread": life.gmail_thread_id,
                ":create_sent": life.create_sent,
                ":change_skipped": life.gmail_change_skipped,
                ":rev": rev,
            },
        )?;
        let old_thread = before.and_then(|(_, thread)| thread);
        self.changed(draft.account, &draft.id, [old_thread, draft.thread])
    }

    pub fn delete(&mut self, id: &DraftId) -> Result<(), StoreError> {
        let Some((account, thread)) = self.placed(id)? else {
            return Ok(());
        };
        self.tx.stamp();
        self.tx
            .db()
            .execute("delete a draft", "DELETE FROM drafts WHERE id = ?1", [id])?;
        self.changed(account, id, [thread, None])
    }

    fn placed(&self, id: &DraftId) -> Result<Option<(AccountId, Option<ThreadId>)>, StoreError> {
        self.tx.db().optional(
            "read where a draft is",
            "SELECT account_id, thread_id FROM drafts WHERE id = ?1",
            [id],
            |r| Ok((r.get("account_id")?, r.get("thread_id")?)),
        )
    }

    fn changed(
        &mut self,
        account: AccountId,
        id: &DraftId,
        threads: [Option<ThreadId>; 2],
    ) -> Result<(), StoreError> {
        self.tx.record().draft(account, id.clone());
        for thread in threads.into_iter().flatten() {
            stamp_thread(self.tx, thread)?;
        }
        Ok(())
    }
}

fn ensure_same_account(
    id: &DraftId,
    stored: AccountId,
    given: AccountId,
) -> Result<(), StoreError> {
    if stored == given {
        return Ok(());
    }
    Err(StoreError::Invalid(format!(
        "draft {id} belongs to account {stored}, not {given}"
    )))
}

pub(crate) fn stamp_thread(tx: &mut WriteTx<'_>, thread: ThreadId) -> Result<(), StoreError> {
    let rev = tx.stamp();
    let account: Option<AccountId> = tx.db().optional(
        "stamp a thread",
        "UPDATE threads SET rev = ?2 WHERE id = ?1 RETURNING account_id",
        (thread, rev),
        |r| r.get("account_id"),
    )?;
    if let Some(account) = account {
        tx.record().thread(account, thread);
    }
    Ok(())
}
