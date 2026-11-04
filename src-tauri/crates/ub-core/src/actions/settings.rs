//! Settings (SPEC Settings): an action, not an undo entry, applied at once.

use ub_domain::AccountId;

use super::{ActionCtx, ActionError};
use crate::views::setting;

/// Sets a setting: remote images globally, or an account's mute. The webview
/// is not trusted, so only those keys and `on` or `off` are taken. The Dock
/// badge follows a mute through the commit's changed settings.
pub(super) fn set_setting(
    ctx: &mut ActionCtx<'_, '_>,
    account: Option<AccountId>,
    key: &str,
    value: &str,
) -> Result<(), ActionError> {
    if value != setting::ON && value != setting::OFF {
        return Err(ActionError::Invalid("a setting is on or off"));
    }
    let account = match (account, key) {
        (None, setting::REMOTE_IMAGES) => AccountId::NONE,
        (Some(account), setting::MUTED) => {
            if ctx.tx.read().accounts().get(account)?.is_none() {
                return Err(ActionError::Invalid("no such account"));
            }
            account
        }
        _ => return Err(ActionError::Invalid("no such setting")),
    };
    ctx.tx.settings().set(account, key, value)?;
    Ok(())
}
