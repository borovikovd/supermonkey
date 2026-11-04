//! Attachments download on demand into the app's data folder and open with
//! their default app (SPEC Reading). They are never fetched during sync.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ub_domain::AttachmentId;
use ub_gmail::{GmailAttachmentId, Priority};
use ub_store::{AttachmentSource, Priority as WritePriority, ReadPool, StoreError};

use super::registry::AccountWorkers;
use crate::{Error, Platform};

/// Downloads an attachment unless its file is on disk already, and opens it.
pub(crate) async fn open_attachment(
    workers: &AccountWorkers,
    pool: &Arc<ReadPool>,
    platform: &dyn Platform,
    attachment: AttachmentId,
) -> Result<(), Error> {
    let path = ensure(workers, pool, attachment).await?;
    platform.open_path(&path)?;
    Ok(())
}

/// The attachment's file, downloaded at priority 2 if it is not on disk.
async fn ensure(
    workers: &AccountWorkers,
    pool: &Arc<ReadPool>,
    attachment: AttachmentId,
) -> Result<PathBuf, Error> {
    let reads = Arc::clone(pool);
    let source =
        tokio::task::spawn_blocking(move || reads.with(|r| r.messages().attachment(attachment)))
            .await??
            .ok_or_else(|| Error::Invalid("The attachment is gone".to_owned()))?;
    let AttachmentSource {
        attachment: record,
        account,
        message_gmail_id,
    } = source;
    if let Some(path) = record.local_path.map(PathBuf::from) {
        let on_disk = path.clone();
        if tokio::task::spawn_blocking(move || on_disk.is_file()).await? {
            return Ok(path);
        }
    }
    let worker = workers
        .get(account)
        .ok_or_else(|| Error::Invalid("The account is offline or signed out".to_owned()))?;
    let gmail_id = record
        .gmail_attachment_id
        .map(GmailAttachmentId::from)
        .ok_or_else(|| Error::Invalid("The attachment has no Gmail id".to_owned()))?;
    let ctx = worker.ctx();
    let bytes = ctx
        .gmail
        .get_attachment(Priority::P2, &message_gmail_id, &gmail_id)
        .await;
    ctx.note(&bytes);
    let bytes = bytes?;
    // Removed meanwhile: its folder is gone and must stay gone.
    if ctx.tasks.token().is_cancelled() {
        return Err(Error::Invalid(
            "The account is offline or signed out".to_owned(),
        ));
    }
    let dir = workers
        .attachments_dir(account)
        .join(message_gmail_id.as_str());
    let name = safe_name(&record.filename);
    let path =
        tokio::task::spawn_blocking(move || write_file(&dir, &name, attachment, &bytes)).await??;
    let stored = path.to_string_lossy().into_owned();
    workers
        .writer()
        .call(WritePriority::Ui, move |tx, _| {
            tx.messages().set_attachment_path(attachment, &stored)
        })
        .await
        .map_err(|e: StoreError| Error::Store(e))?;
    Ok(path)
}

/// Writes the file aside, then renames it into place, so two downloads of
/// one attachment at once never leave a half-written file at its path.
fn write_file(
    dir: &Path,
    name: &str,
    attachment: AttachmentId,
    bytes: &[u8],
) -> Result<PathBuf, std::io::Error> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(dir)?;
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

/// A file name safe to create: no path separators or control characters.
fn safe_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == ':' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    if safe.trim().is_empty() || safe == "." || safe == ".." {
        "attachment".to_owned()
    } else {
        safe
    }
}
