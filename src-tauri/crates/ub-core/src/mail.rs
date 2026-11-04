//! The `Mail` handle (ARCHITECTURE.md, section 8.1): links, attachments,
//! unsubscribe, and block (SPEC Reading, Unsubscribe and block, Compose).
//!
//! Extension points: the unwanted-mail slice fills in [`Mail::unsubscribe`],
//! [`Mail::block`], [`Mail::blocked_senders`], and [`Mail::unblock`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ub_domain::{AccountId, AttachmentId, DraftId, ThreadId};
use ub_ipc::{BlockResult, BlockedSenders, DraftAttachment, Unsubscribed};
use ub_store::ReadPool;
use url::Url;

use crate::workers::{AccountWorkers, open_attachment, safe_name};
use crate::{Error, Platform};

/// Links, attachments, unsubscribe, and block.
#[derive(Clone)]
pub struct Mail {
    platform: Arc<dyn Platform>,
    workers: Arc<AccountWorkers>,
    pool: Arc<ReadPool>,
}

/// What a file the user attaches to a draft is: its name, type, and bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAttachment {
    /// The file's name, as the user picked it.
    pub filename: String,
    /// Its MIME type; empty when the system doesn't know.
    pub mime_type: String,
    /// The file's content.
    pub data: Vec<u8>,
}

impl Mail {
    pub(crate) fn new(
        platform: Arc<dyn Platform>,
        workers: Arc<AccountWorkers>,
        pool: Arc<ReadPool>,
    ) -> Self {
        Self {
            platform,
            workers,
            pool,
        }
    }

    /// Opens a link from a message in the default browser or mail app. Only
    /// web and mail links: the webview is not trusted.
    pub fn open_url(&self, url: &str) -> Result<(), Error> {
        let parsed = Url::parse(url).map_err(|e| Error::Invalid(e.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https" | "mailto") {
            return Err(Error::Invalid(format!(
                "not a web or mail link: {}",
                parsed.scheme()
            )));
        }
        self.platform.open_url(url)?;
        Ok(())
    }

    /// Downloads an attachment if it isn't on disk yet and opens it with its
    /// default app (SPEC Reading).
    pub async fn open_attachment(&self, attachment: AttachmentId) -> Result<(), Error> {
        open_attachment(&self.workers, &self.pool, &*self.platform, attachment).await
    }

    /// Keeps a file the user attached to a draft with the draft's files,
    /// until the draft is sent or discarded (SPEC Compose: attachments).
    /// The composer then saves it as one of the draft's attachments.
    pub async fn add_attachment(
        &self,
        draft: DraftId,
        file: NewAttachment,
    ) -> Result<DraftAttachment, Error> {
        let dir = self.workers.draft_dir(&draft);
        let NewAttachment {
            filename,
            mime_type,
            data,
        } = file;
        let size = i64::try_from(data.len())
            .map_err(|e| Error::Invalid(format!("The file is too large: {e}")))?;
        let name = safe_name(&filename);
        let (id, path) =
            tokio::task::spawn_blocking(move || keep_file(&dir, &name, &data)).await??;
        Ok(DraftAttachment {
            id,
            filename,
            mime_type: if mime_type.is_empty() {
                OCTET_STREAM.to_owned()
            } else {
                mime_type
            },
            size,
            path: Some(path.to_string_lossy().into_owned()),
            source: None,
        })
    }

    /// Unsubscribes from the list `thread`'s latest message came from.
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "an extension point: the unwanted-mail slice unsubscribes here"
    )]
    pub async fn unsubscribe(
        &self,
        account: AccountId,
        thread: ThreadId,
    ) -> Result<Unsubscribed, Error> {
        tracing::debug!(
            account = account.0,
            thread = thread.0,
            "unsubscribe asked for"
        );
        Err(Error::NotYetImplemented("unsubscribing"))
    }

    /// Blocks the sender of `thread`'s latest message, or the sender's
    /// domain, with a Gmail filter.
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "an extension point: the unwanted-mail slice blocks here"
    )]
    pub async fn block(
        &self,
        account: AccountId,
        thread: ThreadId,
        domain: bool,
    ) -> Result<BlockResult, Error> {
        tracing::debug!(
            account = account.0,
            thread = thread.0,
            domain,
            "block asked for"
        );
        Err(Error::NotYetImplemented("blocking senders"))
    }

    /// The account's blocked senders: its Gmail filters that send mail to
    /// Trash.
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "an extension point: the unwanted-mail slice lists filters here"
    )]
    pub async fn blocked_senders(&self, account: AccountId) -> Result<BlockedSenders, Error> {
        tracing::debug!(account = account.0, "blocked senders asked for");
        Err(Error::NotYetImplemented("blocked senders"))
    }

    /// Unblocks a sender by deleting its Gmail filter.
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "an extension point: the unwanted-mail slice deletes the filter here"
    )]
    pub async fn unblock(&self, account: AccountId, filter: &str) -> Result<(), Error> {
        tracing::debug!(account = account.0, filter, "unblock asked for");
        Err(Error::NotYetImplemented("unblocking senders"))
    }
}

/// The type of a file the system has no type for.
const OCTET_STREAM: &str = "application/octet-stream";

/// Writes a draft's file into its own numbered folder inside `dir`, so its
/// name stays as the user picked it. Making the folder reserves the number,
/// so two files added at once never share one. Returns the file's id within
/// the draft and its path.
fn keep_file(dir: &Path, name: &str, data: &[u8]) -> Result<(String, PathBuf), std::io::Error> {
    std::fs::create_dir_all(dir)?;
    for n in 1u32.. {
        let slot = dir.join(n.to_string());
        match std::fs::create_dir(&slot) {
            Ok(()) => {
                let path = slot.join(name);
                std::fs::write(&path, data)?;
                return Ok((format!("file-{n}"), path));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free slot for the file"))
}
