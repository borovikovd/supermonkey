//! The core's error type (ARCHITECTURE.md, section 9). The bridge maps it to
//! the strings the UI reads.

use ub_store::StoreError;

use crate::PlatformError;

/// Why a core call failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database refused a read or write, or the writer has stopped.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A blocking read panicked or was cancelled.
    #[error("a read task failed")]
    ReadTask(#[from] tokio::task::JoinError),
    /// [`App::start`](crate::App::start) ran outside a tokio runtime.
    #[error("the core needs a tokio runtime")]
    NoRuntime(#[from] tokio::runtime::TryCurrentError),
    /// The caller's input is not acceptable; the message says why, in words
    /// the UI can show.
    #[error("{0}")]
    Invalid(String),
    /// The operating system refused a request.
    #[error(transparent)]
    Platform(#[from] PlatformError),
    /// A feature a later slice of the rewrite adds (ARCHITECTURE.md, section
    /// 12); it names the feature.
    #[error("{0} is not implemented yet")]
    NotYetImplemented(&'static str),
}
