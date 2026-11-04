//! Removing an account (SPEC Accounts, Settings).

use ub_domain::AccountId;

use super::{ActionCtx, ActionError};
use crate::effects::Effect;

/// `removeAccount`: deletes the account's rows, its queue and outbox
/// among them, in this transaction. Its worker stops, and its refresh token
/// is revoked and deleted, once the commit lands. Its lists and summary go
/// empty with the commit's patches.
pub(super) fn remove_account(
    ctx: &mut ActionCtx<'_, '_>,
    account: AccountId,
) -> Result<(), ActionError> {
    let Some(record) = ctx.tx.read().accounts().get(account)? else {
        return Err(ActionError::Invalid("no such account"));
    };
    ctx.tx.accounts().remove(account)?;
    ctx.state.effect(Effect::AccountRemoved {
        account,
        email: record.email,
    });
    Ok(())
}
