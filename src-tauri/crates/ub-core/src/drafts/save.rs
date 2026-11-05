//! A composer's later saves, and the Gmail upsert they queue (SPEC Drafts).

use ub_domain::{AccountId, DraftId, MessageId, Millis};
use ub_ipc::DraftFields;
use ub_store::{DraftAttachment, NewOp, OpKind, Reads, StoreError, WriteTx};

use super::close::is_untouched;
use super::text::snippet;
use crate::effects::Effect;
use crate::writer::WriterState;

/// Stores the composer's fields after an edit, and queues the upsert that
/// writes them to Gmail unless the draft is untouched. A draft gone or
/// being sent drops the save: it arrived after the send or discard.
pub(crate) fn update(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    id: &DraftId,
    fields: DraftFields,
    now: Millis,
) -> Result<(), StoreError> {
    let Some(record) = tx.read().drafts().get(id)? else {
        return Ok(());
    };
    if record.in_outbox {
        return Ok(());
    }
    let mut draft = record.draft;
    let DraftFields {
        from,
        to,
        cc,
        bcc,
        subject,
        body_html,
        attachments,
    } = fields;
    // The composer holds attachments as it first loaded them; a file stored
    // since (a download of a Gmail draft's parts) is kept.
    let attachments = attachments
        .into_iter()
        .map(|a| {
            let stored = draft.attachments.iter().find(|s| s.id == a.id);
            DraftAttachment {
                path: a.path.or_else(|| stored.and_then(|s| s.path.clone())),
                id: a.id,
                filename: a.filename,
                mime_type: a.mime_type,
                size: a.size,
                source: a.source,
            }
        })
        .collect();
    draft.from = from.map(Into::into);
    draft.to = to.into_iter().map(Into::into).collect();
    draft.cc = cc.into_iter().map(Into::into).collect();
    draft.bcc = bcc.into_iter().map(Into::into).collect();
    draft.subject = subject;
    draft.snippet = snippet(&body_html);
    draft.body_html = body_html;
    draft.attachments = attachments;
    draft.updated_at = now;
    // An edit made after a Gmail change was skipped wins over it (last
    // writer wins), so that change is not read on close.
    draft.pending_gmail_read = false;
    tx.drafts().put(&draft)?;
    if !is_untouched(&draft) {
        queue_upsert(tx, state, draft.account, id, now)?;
    }
    Ok(())
}

/// Queues one upsert of the draft. It writes the draft as it is when it
/// runs, so a save replaces the one queued: an upsert already running read
/// the draft before this save, and the new one, last in the queue, carries
/// it (DEVIATIONS: Compose).
pub(crate) fn queue_upsert(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    draft: &DraftId,
    now: Millis,
) -> Result<(), StoreError> {
    tx.ops().delete_draft_upserts(draft)?;
    tx.ops().push(&NewOp {
        account,
        action: None,
        kind: OpKind::DraftUpsert,
        draft: Some(draft),
        gmail_draft: None,
        payload: &(),
        created_at: now,
    })?;
    state.effect(Effect::Ops(account));
    Ok(())
}

/// A message's attachments as a draft carries them: a forward carries its
/// target's, and a draft read from Gmail its own. Inline parts stay with
/// the HTML that shows them.
pub(crate) fn message_attachments(
    reads: Reads<'_>,
    message: MessageId,
) -> Result<Vec<DraftAttachment>, StoreError> {
    Ok(reads
        .messages()
        .attachments(message)?
        .into_iter()
        .filter(|a| !a.is_inline)
        .map(|a| DraftAttachment {
            id: format!("src-{}", a.id.0),
            filename: a.filename,
            mime_type: a.mime_type,
            size: a.size,
            path: a.local_path,
            source: Some(a.id),
        })
        .collect())
}
