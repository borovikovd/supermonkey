use sm_domain::AccountId;

use super::{ActionCtx, ActionError};
use crate::effects::Effect;

pub(super) fn remove_account(
    ctx: &mut ActionCtx<'_, '_>,
    account: AccountId,
) -> Result<(), ActionError> {
    let Some(record) = ctx.tx.read().accounts().get(account)? else {
        return Err(ActionError::Invalid("That account was already removed"));
    };
    let drafts = ctx.tx.read().drafts().for_account(account)?;
    ctx.tx.accounts().remove(account)?;
    for record in drafts {
        ctx.state.effect(Effect::DraftFilesGone(record.draft.id));
    }
    ctx.state.effect(Effect::AccountRemoved {
        account,
        email: record.email,
    });
    Ok(())
}
