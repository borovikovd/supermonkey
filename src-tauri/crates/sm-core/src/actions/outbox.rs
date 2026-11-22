use sm_domain::OutboxId;

use super::{ActionCtx, ActionError};

pub(super) fn close_toast(
    ctx: &mut ActionCtx<'_, '_>,
    outbox: OutboxId,
) -> Result<(), ActionError> {
    Ok(crate::outbox::close_toast(ctx.tx, ctx.state, outbox)?)
}
