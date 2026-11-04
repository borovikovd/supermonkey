//! The core's error type (ARCHITECTURE.md, section 9). The bridge maps it to
//! the strings the UI reads.

use ub_store::StoreError;

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
}
