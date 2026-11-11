//! Attachments download on demand into the app's data folder (SPEC
//! Reading): to open one with its default app, and to send one a draft
//! carries (SPEC Send and undo). They are never fetched during sync.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sm_domain::{AttachmentId, GmailMessageId, Millis};
use sm_gmail::{Format, GmailAttachmentId, GmailError, Priority};
use sm_store::{AttachmentSource, Priority as WritePriority, ReadPool, StoreError};

use super::account::AccountCtx;
use super::files::{attachments_dir, safe_name};
use super::quarantine::quarantine;
use super::registry::AccountWorkers;
use crate::{Error, Platform};

/// Why an attachment's file could not be had.
#[derive(Debug, thiserror::Error)]
pub(crate) enum DownloadError {
    /// It is not stored, Gmail no longer serves its bytes, or it is another
    /// account's.
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
    let account = crate::read::read(pool, move |r| r.messages().attachment(attachment))
        .await?
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
    if let Some(path) = record.local_path.map(PathBuf::from)
        && let Some(on_disk) = tokio::task::spawn_blocking(move || path.is_file().then_some(path))
            .await
            .map_err(Error::from)?
    {
        return Ok(on_disk);
    }
    let bytes = match record.gmail_attachment_id.map(GmailAttachmentId::from) {
        Some(gmail_id) => {
            let bytes = ctx
                .gmail
                .get_attachment(Priority::P2, &message_gmail_id, &gmail_id)
                .await;
            ctx.note(&bytes);
            bytes?
        }
        None => in_message(ctx, &message_gmail_id, &record.part_id).await?,
    };
    // Removed meanwhile: its folder is gone and must stay gone.
    if ctx.tasks.token().is_cancelled() {
        return Err(DownloadError::Stopped);
    }
    let dir = attachments_dir(&ctx.deps.data_dir, account).join(message_gmail_id.as_str());
    let name = safe_name(&record.filename);
    let now = ctx.clock().now();
    let path =
        tokio::task::spawn_blocking(move || write_file(&dir, &name, attachment, &bytes, now))
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

/// The bytes of a part Gmail serves inside its message, with no attachment
/// id, as it does for some small files: the message is read again.
async fn in_message(
    ctx: &AccountCtx,
    message: &GmailMessageId,
    part_id: &str,
) -> Result<Vec<u8>, DownloadError> {
    let read = ctx
        .gmail
        .get_message(Priority::P2, message, Format::Full)
        .await;
    ctx.note(&read);
    let Some(payload) = read?.payload else {
        return Err(DownloadError::Gone);
    };
    let parsed = sm_mime::parse_message(&payload).map_err(|e| {
        tracing::warn!(error = %e, "a message read for its attachment doesn't parse");
        DownloadError::Gone
    })?;
    parsed
        .parts
        .into_iter()
        .find(|part| part.id == part_id)
        .and_then(|part| part.data)
        .ok_or(DownloadError::Gone)
}

/// Writes an attachment of the message whose folder is `message_dir` into
/// its own folder there, so two parts with one name (`image.png`,
/// `ATT00001.htm`) never share a file and the name stays as sent. Writes
/// aside, quarantined as downloaded at `now`, then renames into place, so
/// two downloads of one attachment at once never leave a half-written file
/// at its path, and no file from mail is ever there without the flag.
fn write_file(
    message_dir: &Path,
    name: &str,
    attachment: AttachmentId,
    bytes: &[u8],
    now: Millis,
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
    if let Err(e) = quarantine(&part, now) {
        // Never left where it could open unflagged.
        if let Err(removed) = std::fs::remove_file(&part) {
            tracing::warn!(error = %removed, "couldn't remove an attachment that could not be quarantined");
        }
        return Err(e);
    }
    std::fs::rename(&part, &path)?;
    Ok(path)
}
