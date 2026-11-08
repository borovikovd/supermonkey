//! Closing a composer and discarding a draft (SPEC UI state, Actions;
//! Drafts).

use sm_domain::{DraftId, Millis};
use sm_store::{NewOp, OpKind, StoreError, WriteTx};

use super::lifecycle::{self, GmailWrite};
use crate::effects::Effect;
use crate::writer::WriterState;

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
    if lifecycle::discards_on_close(&draft) {
        return discard(tx, state, id, now);
    }
    if lifecycle::composer_closed(&mut draft) {
        tx.drafts().put(&draft)?;
    }
    if lifecycle::has_skipped_gmail_change(&draft) {
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
    // With no Gmail id but a `drafts.create` that went out unanswered, Gmail
    // may hold a copy anyway: the delete finds it by its header.
    let gmail = match lifecycle::gmail_write(&draft) {
        GmailWrite::Update(gmail) => Some(Some(gmail)),
        GmailWrite::Recover => Some(None),
        GmailWrite::Create => None,
    };
    if let Some(gmail) = gmail {
        tx.ops().push(&NewOp {
            account: draft.account,
            action: None,
            kind: OpKind::DraftDelete,
            draft: Some(id),
            gmail_draft: gmail,
            payload: &(),
            created_at: now,
        })?;
        state.effect(Effect::Ops(draft.account));
    }
    let copies = tx.read().messages().draft_copies(draft.account, id)?;
    tx.messages().delete(&copies)?;
    state.draft_gone(draft.account, id.clone());
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
    for id in tx.read().drafts().fresh()? {
        close(tx, state, &id, now)?;
    }
    Ok(())
}
