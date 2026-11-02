//! Google sign-in. Filled in with sync.

use tauri::AppHandle;

use crate::model::*;

pub async fn add_account(_app: &AppHandle) -> anyhow::Result<AccountId> {
    anyhow::bail!("sign-in is not built yet")
}

pub async fn sign_in_again(_app: &AppHandle, _account: AccountId) -> anyhow::Result<()> {
    anyhow::bail!("sign-in is not built yet")
}
