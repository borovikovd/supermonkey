use std::collections::HashSet;
use std::sync::Arc;

use sm_domain::{
    AccountId, DraftId, DraftKind, GmailDraftId, GmailThreadId, Millis, NoticeKind, Rev, Thread,
    ThreadId, gmail_draft_aim, gmail_draft_kind,
};
use sm_store::{
    Draft, DraftAttachment, DraftRecord, NewNotice, NewOp, OpKind, Reads, StoreError, WriteTx,
};

use super::lifecycle::{self, DraftsWriting, GmailIds};
use super::save::message_attachments;
use super::text::snippet;
use crate::effects::Effect;
use crate::workers::{ReadMessage, store_draft_message};
use crate::writer::WriterState;

const GONE_ELSEWHERE: &str = "Draft was sent or deleted elsewhere";

#[derive(Debug, Clone)]
pub(crate) struct GmailDraft {
    pub(crate) id: GmailDraftId,
    pub(crate) thread: GmailThreadId,
    pub(crate) message: ReadMessage,
    pub(crate) editor_html: String,
    pub(crate) quote_html: Option<String>,
}

impl GmailDraft {
    fn into_ids(self) -> GmailIds {
        GmailIds {
            draft: self.id,
            message: self.message.message.gmail_id,
            thread: self.thread,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Reconcile {
    pub(crate) account: AccountId,
    pub(crate) live: Arc<HashSet<GmailDraftId>>,
    pub(crate) started: Rev,
    pub(crate) writing: Arc<DraftsWriting>,
}

impl Reconcile {
    fn moved(&self, draft: &DraftId, rev: Rev) -> bool {
        self.writing.is_busy(draft) || rev > self.started
    }
}

pub(crate) fn store_ids(
    tx: &mut WriteTx<'_>,
    local: &DraftId,
    ids: GmailIds,
) -> Result<bool, StoreError> {
    let Some(record) = tx.read().drafts().get(local)? else {
        return Ok(false);
    };
    let mut d = record.draft;
    lifecycle::written(&mut d, ids);
    tx.drafts().put(&d)?;
    Ok(true)
}

pub(crate) fn delete_gmail_draft(
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

pub(crate) fn drop_gone(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    gone: Vec<DraftId>,
    reconcile: &Reconcile,
    now: Millis,
) -> Result<(), StoreError> {
    for id in gone {
        let Some(record) = tx.read().drafts().get(&id)? else {
            continue;
        };
        let still_gone =
            lifecycle::gmail_draft(&record.draft).is_some_and(|g| !reconcile.live.contains(g));
        if !still_gone || record.in_outbox || reconcile.moved(&id, record.rev) {
            continue;
        }
        let mut draft = record.draft;
        let open = state.subs.has_composer(&id);
        if open || tx.read().ops().upserts_draft(&id)? {
            lifecycle::gmail_gone(&mut draft);
            tx.drafts().put(&draft)?;
            if open {
                tx.notices().add(&NewNotice {
                    account: draft.account,
                    kind: NoticeKind::DraftGone,
                    message: GONE_ELSEWHERE.to_owned(),
                    draft: Some(draft.id),
                    thread: None,
                    created_at: now,
                })?;
            }
        } else {
            tx.drafts().delete(&id)?;
            state.effect(Effect::DraftFilesGone(id));
        }
    }
    Ok(())
}

pub(crate) fn defer_if_open(
    tx: &mut WriteTx<'_>,
    state: &WriterState,
    id: &DraftId,
) -> Result<bool, StoreError> {
    if !state.subs.has_composer(id) {
        return Ok(false);
    }
    if let Some(record) = tx.read().drafts().get(id)? {
        let mut draft = record.draft;
        if lifecycle::gmail_changed(&mut draft) {
            tx.drafts().put(&draft)?;
        }
    }
    Ok(true)
}

pub(crate) fn apply_gmail_draft(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    gmail: GmailDraft,
    reconcile: &Reconcile,
    now: Millis,
) -> Result<(), StoreError> {
    let account = reconcile.account;
    let reads = tx.read();
    if is_settled_elsewhere(reads, &gmail, reconcile)? {
        return Ok(());
    }
    let header = gmail.message.message.sm_draft.as_ref();
    let existing = local_for(reads, account, &gmail.id, header)?;
    if existing.is_none()
        && let Some(local) = header
        && state.is_draft_gone(account, local)
    {
        return delete_gmail_draft(tx, state, account, &gmail.id, now);
    }
    let existing = match existing {
        Some(record) => {
            let local = &record.draft;
            if record.in_outbox || reconcile.moved(&local.id, record.rev) {
                return Ok(());
            }
            if lifecycle::gmail_draft(local) != Some(&gmail.id) {
                let other_live = lifecycle::gmail_draft(local)
                    .is_some_and(|other| reconcile.live.contains(other));
                if other_live {
                    return delete_gmail_draft(tx, state, account, &gmail.id, now);
                }
                let mut adopted = record.draft;
                lifecycle::written(&mut adopted, gmail.into_ids());
                return tx.drafts().put(&adopted);
            }
            if tx.read().ops().upserts_draft(&local.id)? {
                return Ok(());
            }
            if defer_if_open(tx, state, &local.id)? {
                return Ok(());
            }
            Some(record)
        }
        None => None,
    };
    save_gmail_draft(tx, state, account, gmail, existing)
}

fn save_gmail_draft(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    gmail: GmailDraft,
    existing: Option<DraftRecord>,
) -> Result<(), StoreError> {
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
    let mut draft = if let Some(record) = existing {
        record.draft
    } else {
        let draft = new_gmail_draft(reads, account, &gmail, thread)?;
        state.effect(Effect::DraftBack(draft.id.clone()));
        draft
    };
    take_gmail_content(&mut draft, gmail, thread, attachments);
    tx.drafts().put(&draft)
}

fn take_gmail_content(
    draft: &mut Draft,
    gmail: GmailDraft,
    thread: ThreadId,
    attachments: Vec<DraftAttachment>,
) {
    let GmailDraft {
        id,
        thread: gmail_thread,
        message: ReadMessage { message, .. },
        editor_html,
        quote_html,
    } = gmail;
    let ids = GmailIds {
        draft: id,
        message: message.gmail_id,
        thread: gmail_thread,
    };
    lifecycle::read_from_gmail(draft, ids, !attachments.is_empty());
    draft.thread = (draft.kind != DraftKind::New).then_some(thread);
    draft.from = Some(message.from);
    draft.to = message.to;
    draft.cc = message.cc;
    draft.bcc = message.bcc;
    draft.subject = message.subject;
    draft.snippet = snippet(&editor_html);
    draft.body_html = editor_html;
    draft.quote_html = quote_html;
    draft.attachments = attachments;
    draft.updated_at = message.date;
}

fn is_settled_elsewhere(
    reads: Reads<'_>,
    gmail: &GmailDraft,
    reconcile: &Reconcile,
) -> Result<bool, StoreError> {
    let account = reconcile.account;
    if reads.ops().deletes_gmail_draft(account, &gmail.id)? {
        return Ok(true);
    }
    let header = gmail.message.message.sm_draft.as_ref();
    if let Some(local) = header
        && reads.ops().deletes_copies_of(account, local)?
    {
        return Ok(true);
    }
    Ok(header.is_some_and(|d| reconcile.writing.is_busy(d)))
}

fn local_for(
    reads: Reads<'_>,
    account: AccountId,
    gmail: &GmailDraftId,
    header: Option<&DraftId>,
) -> Result<Option<DraftRecord>, StoreError> {
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

fn new_gmail_draft(
    reads: Reads<'_>,
    account: AccountId,
    gmail: &GmailDraft,
    thread: ThreadId,
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
    let id = match &message.sm_draft {
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
