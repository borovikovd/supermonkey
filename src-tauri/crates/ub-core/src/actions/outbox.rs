//! Send toasts (SPEC Send and undo): closing one needs no Gmail.

use ub_domain::OutboxId;

use super::{ActionCtx, ActionError};

/// The user closed a send toast. A sent row's undo entry goes with it, so a
/// stray `z` never trashes delivered mail, and a sent row whose message the
/// thread now holds is done: the row and its draft go.
pub(super) fn close_toast(
    ctx: &mut ActionCtx<'_, '_>,
    outbox: OutboxId,
) -> Result<(), ActionError> {
    Ok(crate::outbox::close_toast(ctx.tx, ctx.state, outbox)?)
}
