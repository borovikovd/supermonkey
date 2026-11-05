//! Closing a composer and discarding a draft (SPEC UI state, Actions;
//! Drafts).

use ub_domain::{DraftId, Millis};
use ub_store::{Draft, NewOp, OpKind, StoreError, WriteTx};

use super::text::is_blank;
use crate::effects::Effect;
use crate::writer::WriterState;

/// Whether the draft is as its composer started it: the defaults' From,
/// recipients, subject, and attachments, and the same body or a blank one.
/// A draft with no defaults (read from Gmail, or brought back by an undo)
/// is never untouched.
pub(crate) fn is_untouched(draft: &Draft) -> bool {
    draft
        .defaults
        .as_ref()
        .is_some_and(|d| d.is_untouched_by(&draft.content(), is_blank(&draft.body_html)))
}

/// A composer closed, as a second `Esc` closes it: a draft started fresh
/// and untouched is discarded, here and on Gmail; any other loses its
/// started-fresh flag. A Gmail change skipped while it was open is read now,
/// unless an edit saved since cleared it.
pub(crate) fn close(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    id: &DraftId,
    now: Millis,
) -> Result<(), StoreError> {
    let Some(record) = tx.read().drafts().get(id)? else {
        return Ok(());
    };
    if record.in_outbox {
        return Ok(());
    }
    let mut draft = record.draft;
    if draft.started_fresh && is_untouched(&draft) {
        return discard(tx, state, id, now);
    }
    if draft.started_fresh {
        draft.started_fresh = false;
        tx.drafts().put(&draft)?;
    }
    if draft.pending_gmail_read {
        state.effect(Effect::ReadDraft {
            account: draft.account,
            draft: draft.id,
        });
    }
    Ok(())
}

/// Deletes the draft here and its Gmail draft through the queue, with its
/// queued upserts, the Gmail draft messages that carry it, and its files.
/// A draft being sent stays: the outbox owns it.
pub(crate) fn discard(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    id: &DraftId,
    now: Millis,
) -> Result<(), StoreError> {
    let Some(record) = tx.read().drafts().get(id)? else {
        return Ok(());
    };
    if record.in_outbox {
        return Ok(());
    }
    let draft = record.draft;
    tx.ops().delete_draft_upserts(id)?;
    tx.drafts().delete(id)?;
    if let Some(gmail) = &draft.gmail_draft_id {
        tx.ops().push(&NewOp {
            account: draft.account,
            action: None,
            kind: OpKind::DraftDelete,
            draft: Some(id),
            gmail_draft: Some(gmail),
            payload: &(),
            created_at: now,
        })?;
        state.effect(Effect::Ops(draft.account));
    }
    let copies = tx.read().messages().draft_copies(draft.account, id)?;
    tx.messages().delete(&copies)?;
    state.effect(Effect::DraftFilesGone(id.clone()));
    Ok(())
}

/// On launch: the untouched rule for every draft whose first composer never
/// closed, after a crash or a quit that cut it off.
pub(crate) fn on_launch(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    now: Millis,
) -> Result<(), StoreError> {
    for id in tx.read().drafts().started_fresh()? {
        close(tx, state, &id, now)?;
    }
    Ok(())
}
