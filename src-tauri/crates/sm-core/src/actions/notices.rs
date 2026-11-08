//! Notices: Gmail rejections and send results the UI shows as toasts
//! (SPEC UI state, Subscriptions).

use sm_domain::NoticeId;

use super::{ActionCtx, ActionError};

/// The UI showed a notice: it leaves the account summary. A notice already
/// gone is not an error.
pub(super) fn ack_notice(ctx: &mut ActionCtx<'_, '_>, notice: NoticeId) -> Result<(), ActionError> {
    ctx.tx.notices().delete(notice)?;
    Ok(())
}
