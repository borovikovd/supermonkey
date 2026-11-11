//! The `Mail` handle (ARCHITECTURE.md, section 8.1): links, attachments,
//! unsubscribe, and block (SPEC Reading, Unsubscribe and block, Compose).

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sm_domain::{AccountId, AttachmentId, DraftId, ThreadId};
use sm_ipc::{BlockResult, BlockedSenders, DraftAttachment, Unsubscribed};
use sm_store::ReadPool;
use url::Url;

use sm_gmail::FilterId;

use crate::workers::{AccountWorker, AccountWorkers, open_attachment, safe_name, unwanted};
use crate::{Error, Platform};

/// Links, attachments, unsubscribe, and block.
pub struct Mail {
    platform: Arc<dyn Platform>,
    workers: Arc<AccountWorkers>,
    pool: Arc<ReadPool>,
}

/// Its name only: it holds the core's handles and the platform, none of them a value to print.
impl fmt::Debug for Mail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mail").finish_non_exhaustive()
    }
}

/// The largest file a draft takes: Gmail's own limit for attachments
/// (SPEC Drafts). Checked before a file is decoded or written.
pub const MAX_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;

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
        // The URL as parsed, so what opens is what was checked.
        self.platform.open_url(parsed.as_str())?;
        Ok(())
    }

    /// Downloads an attachment if it isn't on disk yet and opens it with its
    /// default app (SPEC Reading).
    pub async fn open_attachment(&self, attachment: AttachmentId) -> Result<(), Error> {
        open_attachment(&self.workers, &self.pool, &*self.platform, attachment).await
    }

    /// Keeps a file the user attached to a draft with the draft's files,
    /// until the draft is sent or discarded (SPEC Compose: attachments).
    /// The composer then saves it as one of the draft's attachments. A draft
    /// already gone keeps nothing: [`Error::DraftGone`].
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
        if data.len() > MAX_ATTACHMENT_BYTES {
            return Err(attachment_too_large());
        }
        let size = i64::try_from(data.len())
            .map_err(|e| Error::Invalid(format!("The file is too large: {e}")))?;
        let name = safe_name(&filename);
        let (id, path) =
            tokio::task::spawn_blocking(move || keep_file(&dir, &name, &data)).await??;
        // The draft went while the file was on its way, and its folder with
        // it (SPEC Drafts): this file goes too.
        if self.workers.draft_files_gone(&draft) {
            self.workers.delete_draft_files(draft);
            return Err(Error::DraftGone);
        }
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

    /// Unsubscribes from the list `thread`'s latest message came from (SPEC
    /// Unsubscribe and block). Needs the network; nothing is queued.
    pub async fn unsubscribe(
        &self,
        account: AccountId,
        thread: ThreadId,
    ) -> Result<Unsubscribed, Error> {
        let worker = self.worker(account)?;
        unwanted::unsubscribe(worker.ctx(), self.workers.http(), &*self.platform, thread).await
    }

    /// Blocks the sender of `thread`'s latest message, or the sender's
    /// domain, with a Gmail filter.
    pub async fn block(
        &self,
        account: AccountId,
        thread: ThreadId,
        domain: bool,
    ) -> Result<BlockResult, Error> {
        let worker = self.worker(account)?;
        unwanted::block(worker.ctx(), thread, domain).await
    }

    /// The account's blocked senders: its Gmail filters that send mail to
    /// Trash.
    pub async fn blocked_senders(&self, account: AccountId) -> Result<BlockedSenders, Error> {
        let worker = self.worker(account)?;
        unwanted::blocked_senders(worker.ctx()).await
    }

    /// Unblocks a sender by deleting its Gmail filter, named by its id.
    pub async fn unblock(&self, account: AccountId, filter: String) -> Result<(), Error> {
        let filter = FilterId::from(filter);
        let worker = self.worker(account)?;
        unwanted::unblock(worker.ctx(), &filter).await
    }

    /// The account's running worker: these calls need its Gmail.
    fn worker(&self, account: AccountId) -> Result<AccountWorker, Error> {
        self.workers
            .get(account)
            .ok_or_else(|| Error::Invalid("The account is not signed in".to_owned()))
    }
}

/// Why a file over [`MAX_ATTACHMENT_BYTES`] is refused.
#[must_use]
pub fn attachment_too_large() -> Error {
    Error::Invalid("The file is too large: attachments can be at most 25 MB".to_owned())
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
        #[expect(
            clippy::create_dir,
            reason = "the folder must be new: making it reserves its number, and `create_dir_all` would take one that is there"
        )]
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
