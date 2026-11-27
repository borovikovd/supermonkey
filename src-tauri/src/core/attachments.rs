use std::fs;
use std::path::Path;

use uuid::Uuid;

use crate::domain::{AttachmentId, Millis};
use crate::gmail::Priority as GmailPriority;
use crate::store::{Priority as StorePriority, ReadPool, Writer};

use super::drafts::sanitize_filename;
use super::runtime::Clock;
use super::sync::AccountManager;
use super::writer_state::WriterState;
use super::{CoreError, Platform};

#[expect(
    clippy::too_many_arguments,
    reason = "attachment opener downloads, quarantines, stores path, and opens file via platform"
)]
pub(crate) async fn open_attachment(
    attachment: AttachmentId,
    data_dir: &Path,
    accounts: &AccountManager,
    pool: &ReadPool,
    writer: &Writer<WriterState>,
    clock: &Clock,
    platform: &dyn Platform,
) -> Result<(), CoreError> {
    let Some((account, gmid, rec)) = pool
        .with(|db| db.attachment(attachment))
        .map_err(|e| CoreError::internal(e.to_string()))?
    else {
        return Err(CoreError::other("Attachment not found"));
    };

    if let Some(existing) = &rec.local_path {
        let p = Path::new(existing);
        if p.exists() {
            platform.open_file(p)?;
            return Ok(());
        }
    }

    let Some(gatt_id) = &rec.gmail_att_id else {
        return Err(CoreError::other("Attachment data is unavailable"));
    };
    let client = accounts.gmail_session(account, "Sign in again to open attachments")?;
    client.ensure_online("Couldn't download attachment while offline")?;

    let bytes = client
        .check(
            client
                .gmail()
                .get_attachment(&gmid, gatt_id, GmailPriority::P1)
                .await,
        )
        .await
        .map_err(|err| CoreError::other(format!("Couldn't download attachment: {err}")))?;

    let dir = data_dir.join("attachments");
    fs::create_dir_all(&dir).map_err(|e| CoreError::internal(e.to_string()))?;
    let clean = sanitize_filename(&rec.filename);
    let path = dir.join(format!("{}-{clean}", attachment.0));
    fs::write(&path, &bytes).map_err(|e| CoreError::internal(e.to_string()))?;
    apply_quarantine(&path, clock.now())?;

    let path_str = path.to_string_lossy().into_owned();
    super::log_err(
        "record attachment local path",
        writer
            .call(StorePriority::Sync, move |tx, _state| {
                tx.set_attachment_path(attachment, &path_str)
            })
            .await,
    );

    platform.open_file(&path)?;
    Ok(())
}

pub(crate) fn apply_quarantine(path: &Path, now: Millis) -> Result<(), CoreError> {
    #[cfg(target_os = "macos")]
    {
        let secs = u64::try_from((now.0 / 1000).max(0)).unwrap_or(0);
        let value = format!("0081;{secs:x};Supermonkey;{:X}", Uuid::new_v4());
        xattr::set(path, "com.apple.quarantine", value.as_bytes())
            .map_err(|e| CoreError::internal(format!("quarantine xattr failed: {e}")))?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, now);
    }
    Ok(())
}
