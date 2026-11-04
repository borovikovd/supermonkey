//! Drafts: the composer's state, its defaults, and its Gmail copy's ids
//! (SPEC Drafts, Reply composer).

use rusqlite::{Row, named_params};
use serde::{Deserialize, Serialize};
use ub_domain::{
    AccountId, Addr, AttachmentId, DraftContent, DraftId, DraftKind, GmailDraftId, GmailMessageId,
    GmailThreadId, MessageId, Millis, Rev, ThreadId,
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
}

/// A draft's whole stored state. Read it, change it, and write it back with
/// `DraftWrites::put`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent stored flags of one draft"
)]
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
    /// Its first composer is still open (SPEC Actions).
    pub started_fresh: bool,
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
    /// The content its composer started with, for the untouched check.
    pub defaults: Option<DraftContent>,
    /// The `In-Reply-To` it sends.
    pub in_reply_to: Option<String>,
    /// The `References` it sends.
    pub references: String,
    /// Gmail's draft id from the last upsert.
    pub gmail_draft_id: Option<GmailDraftId>,
    /// Gmail's message id from the last upsert.
    pub gmail_message_id: Option<GmailMessageId>,
    /// Gmail's thread id from the last upsert.
    pub gmail_thread_id: Option<GmailThreadId>,
    /// A Gmail change was skipped while a composer had it open.
    pub pending_gmail_read: bool,
    /// Read from Gmail; its parts are not downloaded yet.
    pub needs_parts: bool,
    /// The last edit; the Drafts list sorts by it.
    pub updated_at: Millis,
}

impl Draft {
    /// An empty draft, as a fresh composer's first save starts it.
    pub fn new(id: DraftId, account: AccountId, kind: DraftKind, now: Millis) -> Self {
        Self {
            id,
            account,
            kind,
            reply_all: false,
            thread: None,
            target: None,
            started_on_latest: false,
            started_fresh: false,
            from: None,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body_html: String::new(),
            snippet: String::new(),
            quote_html: None,
            attachments: Vec::new(),
            defaults: None,
            in_reply_to: None,
            references: String::new(),
            gmail_draft_id: None,
            gmail_message_id: None,
            gmail_thread_id: None,
            pending_gmail_read: false,
            needs_parts: false,
            updated_at: now,
        }
    }

    /// The fields the untouched rule compares, as they are now.
    pub fn content(&self) -> DraftContent {
        DraftContent {
            from: self.from.clone(),
            to: self.to.clone(),
            cc: self.cc.clone(),
            bcc: self.bcc.clone(),
            subject: self.subject.clone(),
            body_html: self.body_html.clone(),
            attachments: self.attachments.iter().map(|a| a.id.clone()).collect(),
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
    let defaults = json_column_opt(r, "defaults")?;
    Ok(DraftRecord {
        draft: Draft {
            id: r.get("id")?,
            account: r.get("account_id")?,
            kind: r.get("kind")?,
            reply_all: r.get("reply_all")?,
            thread: r.get("thread_id")?,
            target: r.get("target_message_id")?,
            started_on_latest: r.get("started_on_latest")?,
            started_fresh: r.get("started_fresh")?,
            from: json_column(r, "from_addr")?,
            to: json_column(r, "to_addrs")?,
            cc: json_column(r, "cc_addrs")?,
            bcc: json_column(r, "bcc_addrs")?,
            subject: r.get("subject")?,
            body_html: r.get("body_html")?,
            snippet: r.get("snippet")?,
            quote_html: r.get("quote_html")?,
            attachments: json_column(r, "attachments")?,
            defaults,
            in_reply_to: r.get("in_reply_to")?,
            references: r.get("references_hdr")?,
            gmail_draft_id: r.get("gmail_draft_id")?,
            gmail_message_id: r.get("gmail_message_id")?,
            gmail_thread_id: r.get("gmail_thread_id")?,
            pending_gmail_read: r.get("pending_gmail_read")?,
            needs_parts: r.get("needs_parts")?,
            updated_at: r.get("updated_at")?,
        },
        rev: r.get("rev")?,
        in_outbox: r.get("in_outbox")?,
    })
}

/// Reads of drafts.
#[derive(Clone, Copy)]
pub struct DraftReads<'c> {
    db: Db<'c>,
}

impl<'c> DraftReads<'c> {
    pub(crate) fn new(db: Db<'c>) -> Self {
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

    /// Drafts whose first composer never closed, for the untouched rule on
    /// launch.
    pub fn started_fresh(self) -> Result<Vec<DraftId>, StoreError> {
        self.db.all(
            "read the fresh drafts",
            "SELECT id FROM drafts WHERE started_fresh = 1 ORDER BY id",
            [],
            |r| r.get("id"),
        )
    }
}

/// Writes to drafts. Each stamps the draft and the threads it is on.
pub struct DraftWrites<'t, 'c> {
    tx: &'t mut WriteTx<'c>,
}

impl<'t, 'c> DraftWrites<'t, 'c> {
    pub(crate) fn new(tx: &'t mut WriteTx<'c>) -> Self {
        Self { tx }
    }

    /// Writes a draft's whole state, adding it if new. A draft can't move to
    /// another account.
    pub fn put(&mut self, draft: &Draft) -> Result<(), StoreError> {
        let before = self.tx.read().drafts().get(&draft.id)?;
        if let Some(before) = &before
            && before.draft.account != draft.account
        {
            return Err(StoreError::Invalid(format!(
                "draft {} belongs to account {}, not {}",
                draft.id, before.draft.account, draft.account
            )));
        }
        let rev = self.tx.stamp();
        let defaults = draft
            .defaults
            .as_ref()
            .map(|d| to_json("defaults", d))
            .transpose()?;
        self.tx.db().execute(
            "store a draft",
            "INSERT INTO drafts (id, account_id, kind, reply_all, thread_id, target_message_id,
                 started_on_latest, started_fresh, from_addr, to_addrs, cc_addrs, bcc_addrs,
                 subject, body_html, snippet, quote_html, attachments, defaults, in_reply_to,
                 references_hdr, gmail_draft_id, gmail_message_id, gmail_thread_id,
                 pending_gmail_read, needs_parts, updated_at, rev)
             VALUES (:id, :account, :kind, :reply_all, :thread, :target, :on_latest, :fresh,
                 :from, :to, :cc, :bcc, :subject, :body, :snippet, :quote, :attachments,
                 :defaults, :in_reply_to, :references, :gmail_draft, :gmail_message,
                 :gmail_thread, :pending_read, :needs_parts, :updated_at, :rev)
             ON CONFLICT (id) DO UPDATE SET
                 kind = excluded.kind, reply_all = excluded.reply_all,
                 thread_id = excluded.thread_id, target_message_id = excluded.target_message_id,
                 started_on_latest = excluded.started_on_latest,
                 started_fresh = excluded.started_fresh, from_addr = excluded.from_addr,
                 to_addrs = excluded.to_addrs, cc_addrs = excluded.cc_addrs,
                 bcc_addrs = excluded.bcc_addrs, subject = excluded.subject,
                 body_html = excluded.body_html, snippet = excluded.snippet,
                 quote_html = excluded.quote_html, attachments = excluded.attachments,
                 defaults = excluded.defaults, in_reply_to = excluded.in_reply_to,
                 references_hdr = excluded.references_hdr,
                 gmail_draft_id = excluded.gmail_draft_id,
                 gmail_message_id = excluded.gmail_message_id,
                 gmail_thread_id = excluded.gmail_thread_id,
                 pending_gmail_read = excluded.pending_gmail_read,
                 needs_parts = excluded.needs_parts, updated_at = excluded.updated_at,
                 rev = excluded.rev",
            named_params! {
                ":id": draft.id,
                ":account": draft.account,
                ":kind": draft.kind,
                ":reply_all": draft.reply_all,
                ":thread": draft.thread,
                ":target": draft.target,
                ":on_latest": draft.started_on_latest,
                ":fresh": draft.started_fresh,
                ":from": to_json("from_addr", &draft.from)?,
                ":to": to_json("to_addrs", &draft.to)?,
                ":cc": to_json("cc_addrs", &draft.cc)?,
                ":bcc": to_json("bcc_addrs", &draft.bcc)?,
                ":subject": draft.subject,
                ":body": draft.body_html,
                ":snippet": draft.snippet,
                ":quote": draft.quote_html,
                ":attachments": to_json("attachments", &draft.attachments)?,
                ":defaults": defaults,
                ":in_reply_to": draft.in_reply_to,
                ":references": draft.references,
                ":gmail_draft": draft.gmail_draft_id,
                ":gmail_message": draft.gmail_message_id,
                ":gmail_thread": draft.gmail_thread_id,
                ":pending_read": draft.pending_gmail_read,
                ":needs_parts": draft.needs_parts,
                ":updated_at": draft.updated_at,
                ":rev": rev,
            },
        )?;
        let old_thread = before.and_then(|b| b.draft.thread);
        self.changed(draft.account, &draft.id, [old_thread, draft.thread])
    }

    /// Deletes a draft. Unknown ids are skipped.
    pub fn delete(&mut self, id: &DraftId) -> Result<(), StoreError> {
        let Some(before) = self.tx.read().drafts().get(id)? else {
            return Ok(());
        };
        self.tx.stamp();
        self.tx
            .db()
            .execute("delete a draft", "DELETE FROM drafts WHERE id = ?1", [id])?;
        self.changed(before.draft.account, id, [before.draft.thread, None])
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
