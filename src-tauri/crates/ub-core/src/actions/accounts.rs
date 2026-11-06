//! Removing an account (SPEC Accounts, Settings).

use ub_domain::AccountId;

use super::{ActionCtx, ActionError};
use crate::effects::Effect;

/// `removeAccount`: deletes the account's rows, its queue, outbox, and
/// drafts among them, in this transaction. Its worker stops, its refresh
/// token is revoked and deleted, and its drafts' files go, once the commit
/// lands. Its lists and summary go
/// empty with the commit's patches.
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
