use sm_domain::NoticeId;

use super::{ActionCtx, ActionError};

pub(super) fn ack_notice(ctx: &mut ActionCtx<'_, '_>, notice: NoticeId) -> Result<(), ActionError> {
    ctx.tx.notices().delete(notice)?;
    Ok(())
}
