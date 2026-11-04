//! The `Mail` handle (ARCHITECTURE.md, section 8.1): links, attachments,
//! unsubscribe, and block (SPEC Reading, Unsubscribe and block, Compose).
//!
//! Extension points: the reading slice fills in [`Mail::open_attachment`],
//! the compose slice [`Mail::add_attachment`], and the unwanted-mail slice
//! [`Mail::unsubscribe`], [`Mail::block`], [`Mail::blocked_senders`], and
//! [`Mail::unblock`].

use std::sync::Arc;

use ub_domain::{AccountId, AttachmentId, DraftId, ThreadId};
use ub_ipc::{BlockResult, BlockedSenders, DraftAttachment, Unsubscribed};
use url::Url;

use crate::{Error, Platform};

/// Links, attachments, unsubscribe, and block.
#[derive(Clone)]
pub struct Mail {
    platform: Arc<dyn Platform>,
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
    pub(crate) fn new(platform: Arc<dyn Platform>) -> Self {
        Self { platform }
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
    /// default app.
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "an extension point: the reading slice downloads the attachment here"
    )]
    pub async fn open_attachment(&self, attachment: AttachmentId) -> Result<(), Error> {
        tracing::debug!(attachment = attachment.0, "open attachment asked for");
        Err(Error::NotYetImplemented("opening attachments"))
    }

    /// Keeps a file the user attached to a draft with the draft's files.
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "an extension point: the compose slice writes the file here"
    )]
    pub async fn add_attachment(
        &self,
        draft: DraftId,
        file: NewAttachment,
    ) -> Result<DraftAttachment, Error> {
        drop((draft, file));
        Err(Error::NotYetImplemented("attaching files"))
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
