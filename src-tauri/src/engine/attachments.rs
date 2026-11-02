//! Attachment downloads and opening. Filled in with the composer.

use tauri::AppHandle;

use crate::model::*;

pub async fn open(_app: &AppHandle, _attachment: i64) -> anyhow::Result<()> {
    anyhow::bail!("attachments are not built yet")
}

pub async fn pick(_app: &AppHandle, _draft: &str) -> anyhow::Result<Vec<DraftAttachment>> {
    anyhow::bail!("attachments are not built yet")
}
