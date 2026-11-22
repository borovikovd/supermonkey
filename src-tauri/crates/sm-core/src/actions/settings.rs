use sm_domain::{AccountId, SettingKey};
use sm_ipc::Setting;

use super::{ActionCtx, ActionError};

pub(super) fn set_setting(
    ctx: &mut ActionCtx<'_, '_>,
    setting: Setting,
) -> Result<(), ActionError> {
    let (account, key, on) = match setting {
        Setting::RemoteImages { on } => (AccountId::NONE, SettingKey::RemoteImages, on),
        Setting::Muted { account, on } => {
            ctx.require_account(account)?;
            (account, SettingKey::Muted, on)
        }
    };
    ctx.tx.settings().set(account, key, on)?;
    Ok(())
}
