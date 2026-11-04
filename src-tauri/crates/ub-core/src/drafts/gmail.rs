//! What Gmail's copy of a draft changes here (SPEC Drafts, Gmail errors):
//! the ids an upsert returns, a Gmail draft read back, a draft deleted on
//! Gmail, and the copies the `X-Ubermensch-Draft` header identifies.

use std::collections::HashSet;
use std::sync::Arc;

use ub_domain::{
    AccountId, DraftId, DraftKind, GmailDraftId, GmailMessageId, GmailThreadId, Millis, NoticeKind,
    Rev, Thread, gmail_draft_aim, gmail_draft_kind,
};
use ub_store::{Draft, NewNotice, NewOp, OpKind, Reads, StoreError, WriteTx};

use super::save::message_attachments;
use super::text::snippet;
use super::writing::DraftsWriting;
use crate::effects::Effect;
use crate::workers::{ReadMessage, store_draft_message};
use crate::writer::WriterState;

/// The toast for a draft open in a composer whose Gmail draft went.
const GONE_ELSEWHERE: &str = "Draft was sent or deleted elsewhere";

/// A Gmail draft as `drafts.get` (`format=full`) gave it, parsed and
/// sanitized off the writer thread.
#[derive(Debug, Clone)]
pub(crate) struct GmailDraft {
    /// Gmail's draft id.
    pub(crate) id: GmailDraftId,
    /// Its Gmail thread.
    pub(crate) thread: GmailThreadId,
    /// Its message, with its body and attachments.
    pub(crate) message: ReadMessage,
    /// The HTML above the `gmail_quote` element: the editor's.
    pub(crate) editor_html: String,
    /// The `gmail_quote` element onward: the quote block.
    pub(crate) quote_html: Option<String>,
}

/// What a draft reconcile knows and since when (DEVIATIONS: Sync). Gmail's
/// answers are only as new as the request: a local draft that changed
/// after the first request, or is being written to Gmail, is left for the
/// next reconcile.
#[derive(Debug, Clone)]
pub(crate) struct Reconcile {
    /// The account's Gmail draft ids, as `drafts.list` gave them.
    pub(crate) live: Arc<HashSet<GmailDraftId>>,
    /// The last `rev` committed before the first request.
    pub(crate) started: Rev,
    /// The account's draft writes.
    pub(crate) writing: Arc<DraftsWriting>,
}

impl Reconcile {
    /// Whether the local draft changed or is being written since the start.
    fn moved(&self, draft: &DraftId, rev: Rev) -> bool {
        self.writing.is_busy(draft) || rev > self.started
    }
}

/// Stores the Gmail ids an upsert returned. With `orphan`, a draft
/// discarded while its upsert was in flight has the Gmail draft that upsert
/// made deleted too, or sync would bring it back.
pub(crate) fn store_ids(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    local: &DraftId,
    ids: (&GmailDraftId, &GmailMessageId, &GmailThreadId),
    orphan: Option<AccountId>,
    now: Millis,
) -> Result<(), StoreError> {
    let (draft, message, thread) = ids;
    let Some(record) = tx.read().drafts().get(local)? else {
        if let Some(account) = orphan {
            delete_gmail_draft(tx, state, account, draft, now)?;
        }
        return Ok(());
    };
    let mut d = record.draft;
    d.gmail_draft_id = Some(draft.clone());
    d.gmail_message_id = Some(message.clone());
    d.gmail_thread_id = Some(thread.clone());
    tx.drafts().put(&d)
}

/// Queues the delete of a Gmail draft no local draft owns, once.
fn delete_gmail_draft(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    gmail: &GmailDraftId,
    now: Millis,
) -> Result<(), StoreError> {
    if tx.read().ops().deletes_gmail_draft(account, gmail)? {
        return Ok(());
    }
    tx.ops().push(&NewOp {
        account,
        action: None,
        kind: OpKind::DraftDelete,
        draft: None,
        gmail_draft: Some(gmail),
        payload: &(),
        created_at: now,
    })?;
    state.effect(Effect::Ops(account));
    Ok(())
}

/// Drafts whose Gmail draft `drafts.list` no longer has (sent from a phone,
/// say) are deleted, unless a composer has one open: then it keeps its
/// text, loses its Gmail ids so its next upsert creates one, and a notice
/// says so. A draft that changed since the list was read is not gone.
pub(crate) fn drop_gone(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    gone: &[DraftId],
    reconcile: &Reconcile,
    now: Millis,
) -> Result<(), StoreError> {
    for id in gone {
        let Some(record) = tx.read().drafts().get(id)? else {
            continue;
        };
        let still_gone = record
            .draft
            .gmail_draft_id
            .as_ref()
            .is_some_and(|g| !reconcile.live.contains(g));
        if !still_gone || record.in_outbox || reconcile.moved(id, record.rev) {
            continue;
        }
        let mut draft = record.draft;
        if state.subs.has_composer(id) {
            draft.gmail_draft_id = None;
            draft.gmail_message_id = None;
            tx.drafts().put(&draft)?;
            tx.notices().add(&NewNotice {
                account: draft.account,
                kind: NoticeKind::DraftGone,
                message: GONE_ELSEWHERE.to_owned(),
                draft: Some(draft.id),
                thread: None,
                created_at: now,
            })?;
        } else {
            tx.drafts().delete(id)?;
            state.effect(Effect::DraftFilesGone(id.clone()));
        }
    }
    Ok(())
}

/// A Gmail change to a draft open in a composer is not read over the text
/// being edited: it is read when the composer closes. Returns whether the
/// draft is open.
pub(crate) fn defer_if_open(
    tx: &mut WriteTx<'_>,
    state: &WriterState,
    id: &DraftId,
) -> Result<bool, StoreError> {
    if !state.subs.has_composer(id) {
        return Ok(false);
    }
    if let Some(record) = tx.read().drafts().get(id)?
        && !record.draft.pending_gmail_read
    {
        let mut draft = record.draft;
        draft.pending_gmail_read = true;
        tx.drafts().put(&draft)?;
    }
    Ok(true)
}

/// Creates or updates the local draft for a Gmail draft (SPEC Drafts, Reply
/// composer). One this app wrote is found by its Gmail id, or else by its
/// `X-Ubermensch-Draft` header: then it is the local draft, whose text is
/// kept and gets the Gmail ids, unless that draft already has a live Gmail
/// draft and this is an extra copy, which is deleted. A draft new to the
/// app gets its kind and target once, from its thread's stored messages.
pub(crate) fn apply_gmail_draft(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    gmail: GmailDraft,
    reconcile: &Reconcile,
    now: Millis,
) -> Result<(), StoreError> {
    let reads = tx.read();
    // Discarded here, and its delete has not reached Gmail yet.
    if reads.ops().deletes_gmail_draft(account, &gmail.id)? {
        return Ok(());
    }
    let header = gmail.message.message.ub_draft.clone();
    // A write of the draft it names is in flight: its answer settles it.
    if header
        .as_ref()
        .is_some_and(|d| reconcile.writing.is_busy(d))
    {
        return Ok(());
    }
    let existing = local_for(reads, account, &gmail.id, header.as_ref())?;
    if let Some(record) = &existing {
        let local = &record.draft;
        if record.in_outbox || reconcile.moved(&local.id, record.rev) {
            return Ok(());
        }
        if local.gmail_draft_id.as_ref() != Some(&gmail.id) {
            return match &local.gmail_draft_id {
                Some(other) if reconcile.live.contains(other) => {
                    delete_gmail_draft(tx, state, account, &gmail.id, now)
                }
                _ => {
                    let mut adopted = local.clone();
                    adopted.gmail_draft_id = Some(gmail.id);
                    adopted.gmail_message_id = Some(gmail.message.message.gmail_id);
                    adopted.gmail_thread_id = Some(gmail.thread);
                    tx.drafts().put(&adopted)
                }
            };
        }
        // Opened since the list was read: read once its composer closes.
        if defer_if_open(tx, state, &local.id)? {
            return Ok(());
        }
    }
    let Some(thread) = store_draft_message(tx, state, account, &gmail.thread, &gmail.message)?
    else {
        return Ok(());
    };
    let reads = tx.read();
    let message = &gmail.message.message;
    let attachments = match reads.messages().by_gmail_id(account, &message.gmail_id)? {
        Some(row) => message_attachments(reads, row.id)?,
        None => Vec::new(),
    };
    let mut draft = match existing {
        Some(record) => record.draft,
        None => new_gmail_draft(reads, account, &gmail, thread)?,
    };
    draft.thread = (draft.kind != DraftKind::New).then_some(thread);
    draft.from = Some(message.from.clone());
    draft.to.clone_from(&message.to);
    draft.cc.clone_from(&message.cc);
    draft.bcc.clone_from(&message.bcc);
    draft.subject.clone_from(&message.subject);
    draft.snippet = snippet(&gmail.editor_html);
    draft.body_html = gmail.editor_html;
    draft.quote_html = gmail.quote_html;
    draft.needs_parts = !attachments.is_empty();
    draft.attachments = attachments;
    draft.gmail_draft_id = Some(gmail.id);
    draft.gmail_message_id = Some(message.gmail_id.clone());
    draft.gmail_thread_id = Some(gmail.thread);
    draft.pending_gmail_read = false;
    draft.updated_at = message.date;
    tx.drafts().put(&draft)
}

/// The local draft of a Gmail draft: by its Gmail id, else by the local id
/// its header names, in the same account.
fn local_for(
    reads: Reads<'_>,
    account: AccountId,
    gmail: &GmailDraftId,
    header: Option<&DraftId>,
) -> Result<Option<ub_store::DraftRecord>, StoreError> {
    if let Some(id) = reads.drafts().by_gmail_draft(account, gmail)? {
        return reads.drafts().get(&id);
    }
    let Some(id) = header else {
        return Ok(None);
    };
    Ok(reads
        .drafts()
        .get(id)?
        .filter(|r| r.draft.account == account))
}

/// A draft for a Gmail draft the app has no local draft for: its kind, and
/// the message it is aimed at, from its thread's messages.
fn new_gmail_draft(
    reads: Reads<'_>,
    account: AccountId,
    gmail: &GmailDraft,
    thread: ub_domain::ThreadId,
) -> Result<Draft, StoreError> {
    let message = &gmail.message.message;
    let facts = Thread::new(reads.messages().facts(thread)?);
    let alone = facts.non_drafts().next().is_none();
    let kind = gmail_draft_kind(alone, message.in_reply_to.is_some(), &message.subject);
    let by_reply = match &message.in_reply_to {
        Some(id) => reads
            .messages()
            .by_header_id(thread, id.trim_matches(['<', '>']))?,
        None => None,
    };
    let aim = gmail_draft_aim(&facts, by_reply, message.date);
    // A header naming a draft of another account is not this app's draft.
    let id = match &message.ub_draft {
        Some(id) if reads.drafts().get(id)?.is_none() => id.clone(),
        _ => DraftId::from(format!("g-{}", gmail.id)),
    };
    let mut draft = Draft::new(id, account, kind, message.date);
    if kind != DraftKind::New {
        draft.target = aim.target;
        draft.started_on_latest = aim.started_on_latest;
    }
    draft.in_reply_to.clone_from(&message.in_reply_to);
    draft.references.clone_from(&message.references);
    Ok(draft)
}
