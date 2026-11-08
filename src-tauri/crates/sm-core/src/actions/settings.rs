//! Settings (SPEC Settings): an action, not an undo entry, applied at once.

use sm_domain::{AccountId, SettingKey};
use sm_ipc::Setting;

use super::{ActionCtx, ActionError};

/// Sets a setting: remote images globally, or an account's mute. The Dock
/// badge follows a mute through the commit's changed settings.
pub(super) fn set_setting(
    ctx: &mut ActionCtx<'_, '_>,
    setting: Setting,
) -> Result<(), ActionError> {
    let (account, key, on) = match setting {
        Setting::RemoteImages { on } => (AccountId::NONE, SettingKey::RemoteImages, on),
        Setting::Muted { account, on } => {
            if ctx.tx.read().accounts().get(account)?.is_none() {
                return Err(ActionError::Invalid(
                    "That account is no longer in Supermonkey",
                ));
            }
            (account, SettingKey::Muted, on)
        }
    };
    ctx.tx.settings().set(account, key, on)?;
    Ok(())
}
