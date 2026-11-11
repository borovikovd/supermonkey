//! Drafts: the composer's state and the draft's lifecycle (SPEC Drafts,
//! Reply composer). The lifecycle is plain persistence here: core's draft
//! lifecycle module is the only code that reads or changes it.

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

/// One attachment of a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftAttachment {
    /// Its id within the draft.
    pub id: String,
    /// The file name.
    pub filename: String,
    /// The MIME type.
    pub mime_type: String,
    /// The size in bytes.
    pub size: i64,
    /// The local file, once there is one.
    pub path: Option<String>,
    /// The stored attachment it was carried from (a forward, or a draft
    /// read from Gmail).
    pub source: Option<AttachmentId>,
    /// The `rev` at which Rust added it to a draft its composer had already
    /// loaded (a forward's attachments, once the target's body landed). A
    /// save based on an older `rev` never saw it, so it does not remove it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<Rev>,
}

/// What a draft waits for before it is written to Gmail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftWait {
    /// Read from Gmail: its parts, downloaded.
    Parts,
    /// Filled while its target had no stored body, so it quotes the target's
    /// snippet: the target's body, to quote it whole and carry a forward's
    /// attachments.
    TargetBody,
}

impl DraftWait {
    /// The stored text.
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

/// How far a draft's first composer got, for the untouched rule (SPEC UI
/// state, Actions; Drafts).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DraftStage {
    /// Its first composer is still open. `defaults` is the content it
    /// started with.
    Fresh {
        /// The content its composer started with.
        defaults: DraftContent,
    },
    /// Its first composer closed. `defaults` is the content it started
    /// with.
    Opened {
        /// The content its composer started with.
        defaults: DraftContent,
    },
    /// It has no defaults: the user meant to keep it, or it came from Gmail.
    #[default]
    Kept,
}

impl DraftStage {
    /// The stored text of the `stage` column.
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Fresh { .. } => "fresh",
            Self::Opened { .. } => "opened",
            Self::Kept => "kept",
        }
    }

    /// The `defaults` column.
    const fn defaults(&self) -> Option<&DraftContent> {
        match self {
            Self::Fresh { defaults } | Self::Opened { defaults } => Some(defaults),
            Self::Kept => None,
        }
    }

    /// The stage from its two columns.
    fn from_columns(stage: &str, defaults: Option<DraftContent>) -> rusqlite::Result<Self> {
        match (stage, defaults) {
            ("fresh", Some(defaults)) => Ok(Self::Fresh { defaults }),
            ("opened", Some(defaults)) => Ok(Self::Opened { defaults }),
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

/// A draft's lifecycle as stored: its stage, what it waits for, and its
/// Gmail copy. Core's draft lifecycle module is the only code that reads or
/// changes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DraftLifecycle {
    /// How far its first composer got.
    pub stage: DraftStage,
    /// What it waits for before it is written to Gmail.
    pub waits_for: Option<DraftWait>,
    /// Gmail's draft id from the last write.
    pub gmail_draft_id: Option<GmailDraftId>,
    /// Gmail's message id from the last write.
    pub gmail_message_id: Option<GmailMessageId>,
    /// Gmail's thread id from the last write.
    pub gmail_thread_id: Option<GmailThreadId>,
    /// A `drafts.create` was sent for it and its answer never stored (the
    /// app quit, or the write was cancelled).
    pub create_sent: bool,
    /// A Gmail change was skipped while a composer had it open.
    pub gmail_change_skipped: bool,
}

/// A draft's whole stored state. Read it, change it, and write it back with
/// `DraftWrites::put`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// The local id, made by the UI.
    pub id: DraftId,
    /// Its account; never changes.
    pub account: AccountId,
    /// New, reply, or forward; kept from the start.
    pub kind: DraftKind,
    /// A reply all.
    pub reply_all: bool,
    /// The thread it answers.
    pub thread: Option<ThreadId>,
    /// The message it answers.
    pub target: Option<MessageId>,
    /// Started on the thread's latest message.
    pub started_on_latest: bool,
    /// The From mailbox.
    pub from: Option<Addr>,
    /// The `To` mailboxes.
    pub to: Vec<Addr>,
    /// The `Cc` mailboxes.
    pub cc: Vec<Addr>,
    /// The `Bcc` mailboxes.
    pub bcc: Vec<Addr>,
    /// The subject.
    pub subject: String,
    /// The editor's HTML.
    pub body_html: String,
    /// The body as plain text, for list rows and the outbox.
    pub snippet: String,
    /// The kept quote block.
    pub quote_html: Option<String>,
    /// Its attachments.
    pub attachments: Vec<DraftAttachment>,
    /// The `In-Reply-To` it sends.
    pub in_reply_to: Option<String>,
    /// The `References` it sends.
    pub references: String,
    /// The last edit; the Drafts list sorts by it.
    pub updated_at: Millis,
    /// Its lifecycle, which only core's draft lifecycle module reads or
    /// changes.
    pub lifecycle: DraftLifecycle,
}

impl Draft {
    /// An empty draft, kept, with no Gmail copy.
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

    /// The fields the untouched rule compares, as they are now.
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

/// A stored draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftRecord {
    /// Its state.
    pub draft: Draft,
    /// The `rev` of its last change.
    pub rev: Rev,
    /// It has an outbox row: it is being sent, and shows only as that.
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

/// Reads of drafts.
#[derive(Debug, Clone, Copy)]
pub struct DraftReads<'c> {
    db: Db<'c>,
}

impl<'c> DraftReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// One draft.
    pub fn get(self, id: &DraftId) -> Result<Option<DraftRecord>, StoreError> {
        self.db.optional(
            "read a draft",
            "SELECT d.*, EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = d.id) AS in_outbox
             FROM drafts d WHERE d.id = ?1",
            [id],
            draft_row,
        )
    }

    /// Drafts by id, in the order asked; unknown ids are left out.
    pub fn get_many(self, ids: &[DraftId]) -> Result<Vec<DraftRecord>, StoreError> {
        self.db.all(
            "read drafts",
            "SELECT d.*, EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = d.id) AS in_outbox
             FROM json_each(?1) j JOIN drafts d ON d.id = j.value ORDER BY j.key",
            [id_list(ids)?],
            draft_row,
        )
    }

    /// The Drafts list (SPEC Views): drafts with no outbox row, newest edit
    /// first.
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

    /// A thread's reply and forward drafts with no outbox row, oldest edit
    /// first: the collapsed drafts a conversation shows.
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

    /// Every draft of the account, for reconciling with `drafts.list`.
    pub fn for_account(self, account: AccountId) -> Result<Vec<DraftRecord>, StoreError> {
        self.db.all(
            "read an account's drafts",
            "SELECT d.*, EXISTS (SELECT 1 FROM outbox o WHERE o.draft_id = d.id) AS in_outbox
             FROM drafts d WHERE d.account_id = ?1 ORDER BY d.id",
            [account],
            draft_row,
        )
    }

    /// The local draft whose Gmail copy is `gmail_draft_id`.
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

    /// Drafts whose first composer never closed ([`DraftStage::Fresh`]), for
    /// the untouched rule on launch.
    pub fn fresh(self) -> Result<Vec<DraftId>, StoreError> {
        self.db.all(
            "read the fresh drafts",
            "SELECT id FROM drafts WHERE stage = 'fresh' ORDER BY id",
            [],
            |r| r.get("id"),
        )
    }

    /// Drafts aimed at `message` that wait for its body
    /// ([`DraftWait::TargetBody`]).
    pub fn awaiting_body(self, message: MessageId) -> Result<Vec<DraftId>, StoreError> {
        self.db.all(
            "read the drafts waiting for a message's body",
            "SELECT id FROM drafts WHERE target_message_id = ?1 AND waits_for = ?2 ORDER BY id",
            (message, DraftWait::TargetBody),
            |r| r.get("id"),
        )
    }
}

/// Adds a draft, or replaces every column of the one with its id but the
/// account.
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

/// Writes to drafts. Each stamps the draft and the threads it is on.
#[derive(Debug)]
pub struct DraftWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> DraftWrites<'t, 'c> {
    pub(crate) const fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Writes a draft's whole state, adding it if new. A draft can't move to
    /// another account.
    pub fn put(&mut self, draft: &Draft) -> Result<(), StoreError> {
        let before = self.placed(&draft.id)?;
        if let Some((account, _)) = before
            && account != draft.account
        {
            return Err(StoreError::Invalid(format!(
                "draft {} belongs to account {}, not {}",
                draft.id, account, draft.account
            )));
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

    /// Deletes a draft. Unknown ids are skipped.
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

    /// The account and thread a stored draft is in.
    fn placed(&self, id: &DraftId) -> Result<Option<(AccountId, Option<ThreadId>)>, StoreError> {
        self.tx.db().optional(
            "read where a draft is",
            "SELECT account_id, thread_id FROM drafts WHERE id = ?1",
            [id],
            |r| Ok((r.get("account_id")?, r.get("thread_id")?)),
        )
    }

    /// Records the draft and stamps the threads it was and is on.
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

/// Stamps a thread whose drafts or outbox rows changed, if it still exists.
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
