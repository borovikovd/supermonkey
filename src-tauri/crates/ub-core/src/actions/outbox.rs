//! Send toasts (SPEC Send and undo). Queueing and undoing sends is the
//! compose slice's; closing the toast needs no Gmail.

use ub_domain::{AccountId, OutboxId, OutboxState};

use super::{ActionCtx, ActionError};
use crate::effects::Effect;

/// The user closed a send toast. A sent row's undo entry goes with it, so a
/// stray `z` never trashes delivered mail, and a sent row whose message the
/// thread now holds is done: the row and its draft go. A row already gone
/// is not an error.
pub(super) fn close_toast(
    ctx: &mut ActionCtx<'_, '_>,
    outbox: OutboxId,
) -> Result<(), ActionError> {
    let Some(record) = ctx.tx.read().outbox().get(outbox)? else {
        return Ok(());
    };
    let mut row = record.outbox;
    if !row.toast_closed {
        row.toast_closed = true;
        ctx.tx.outbox().update(&row)?;
    }
    if row.state == OutboxState::Sent {
        ctx.tx.undo().delete_for_outbox(outbox)?;
    }
    finish(ctx, row.account)
}

/// Drops the account's finished sends: sent, toast closed, and the message
/// stored. Their drafts' files go after the commit.
fn finish(ctx: &mut ActionCtx<'_, '_>, account: AccountId) -> Result<(), ActionError> {
    for (outbox, draft) in ctx.tx.read().outbox().finished(account)? {
        ctx.tx.undo().delete_for_outbox(outbox)?;
        ctx.tx.outbox().delete(outbox)?;
        ctx.tx.drafts().delete(&draft)?;
        ctx.state.effect(Effect::DraftFilesGone(draft));
    }
    Ok(())
}
