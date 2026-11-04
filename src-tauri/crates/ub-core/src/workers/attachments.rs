//! Attachments download on demand into the app's data folder (SPEC
//! Reading): to open one with its default app, and to send one a draft
//! carries (SPEC Send and undo). They are never fetched during sync.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ub_domain::AttachmentId;
use ub_gmail::{GmailAttachmentId, GmailError, Priority};
use ub_store::{AttachmentSource, Priority as WritePriority, ReadPool, StoreError};

use super::account::AccountCtx;
use super::files::{attachments_dir, safe_name};
use super::registry::AccountWorkers;
use crate::{Error, Platform};

/// Why an attachment's file could not be had.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DownloadError {
    /// It is not stored, has no Gmail id, or is another account's.
    #[error("The attachment is gone")]
    Gone,
    /// The account stopped syncing: offline at launch, or signed out.
    #[error("The account is offline or signed out")]
    Stopped,
    /// Gmail did not give it.
    #[error(transparent)]
    Gmail(#[from] GmailError),
    /// The database or the disk refused.
    #[error(transparent)]
    Core(#[from] Error),
}

impl From<DownloadError> for Error {
    fn from(e: DownloadError) -> Self {
        match e {
            DownloadError::Gone | DownloadError::Stopped => Self::Invalid(e.to_string()),
            DownloadError::Gmail(e) => Self::Gmail(e),
            DownloadError::Core(e) => e,
        }
    }
}

/// Downloads an attachment unless its file is on disk already, and opens it.
pub(crate) async fn open_attachment(
    workers: &AccountWorkers,
    pool: &Arc<ReadPool>,
    platform: &dyn Platform,
    attachment: AttachmentId,
) -> Result<(), Error> {
    let reads = Arc::clone(pool);
    let account =
        tokio::task::spawn_blocking(move || reads.with(|r| r.messages().attachment(attachment)))
            .await??
            .ok_or(DownloadError::Gone)?
            .account;
    let worker = workers.get(account).ok_or(DownloadError::Stopped)?;
    let path = ensure(worker.ctx(), attachment).await?;
    platform.open_path(&path)?;
    Ok(())
}

/// The file of one of the account's attachments, downloaded at priority 2
/// if it is not on disk.
pub(crate) async fn ensure(
    ctx: &AccountCtx,
    attachment: AttachmentId,
) -> Result<PathBuf, DownloadError> {
    let source = ctx
        .read(move |r| r.messages().attachment(attachment))
        .await?
        .ok_or(DownloadError::Gone)?;
    let AttachmentSource {
        attachment: record,
        account,
        message_gmail_id,
    } = source;
    if account != ctx.account {
        return Err(DownloadError::Gone);
    }
    if let Some(path) = record.local_path.map(PathBuf::from) {
        let on_disk = path.clone();
        if tokio::task::spawn_blocking(move || on_disk.is_file())
            .await
            .map_err(Error::from)?
        {
            return Ok(path);
        }
    }
    let gmail_id = record
        .gmail_attachment_id
        .map(GmailAttachmentId::from)
        .ok_or(DownloadError::Gone)?;
    let bytes = ctx
        .gmail
        .get_attachment(Priority::P2, &message_gmail_id, &gmail_id)
        .await;
    ctx.note(&bytes);
    let bytes = bytes?;
    // Removed meanwhile: its folder is gone and must stay gone.
    if ctx.tasks.token().is_cancelled() {
        return Err(DownloadError::Stopped);
    }
    let dir = attachments_dir(&ctx.deps.data_dir, account).join(message_gmail_id.as_str());
    let name = safe_name(&record.filename);
    let path = tokio::task::spawn_blocking(move || write_file(&dir, &name, attachment, &bytes))
        .await
        .map_err(Error::from)?
        .map_err(Error::from)?;
    let stored = path.to_string_lossy().into_owned();
    ctx.writer()
        .call(WritePriority::Ui, move |tx, _| {
            tx.messages().set_attachment_path(attachment, &stored)
        })
        .await
        .map_err(|e: StoreError| Error::Store(e))?;
    Ok(path)
}

/// Writes an attachment of the message whose folder is `message_dir` into
/// its own folder there, so two parts with one name (`image.png`,
/// `ATT00001.htm`) never share a file and the name stays as sent. Writes
/// aside, then renames into place, so two downloads of one attachment at
/// once never leave a half-written file at its path.
fn write_file(
    message_dir: &Path,
    name: &str,
    attachment: AttachmentId,
    bytes: &[u8],
) -> Result<PathBuf, std::io::Error> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = message_dir.join(attachment.0.to_string());
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    let part = dir.join(format!(
        ".{}-{}.part",
        attachment.0,
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_attachments_with_one_name_keep_their_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_file(dir.path(), "image.png", AttachmentId(1), b"first").unwrap();
        let second = write_file(dir.path(), "image.png", AttachmentId(2), b"second").unwrap();
        assert_ne!(first, second);
        assert_eq!(
            first.file_name(),
            second.file_name(),
            "the name stays as sent"
        );
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        assert_eq!(std::fs::read(&second).unwrap(), b"second");
    }
}
