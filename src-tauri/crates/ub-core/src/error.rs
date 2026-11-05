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
    /// The token store could not be read or written; the message says
    /// which file and why, never a token.
    #[error(transparent)]
    Tokens(#[from] ub_gmail::TokenStoreError),
    /// A Google sign-in did not finish; the message says why.
    #[error("Sign-in failed: {0}")]
    SignIn(#[from] ub_gmail::SignInError),
    /// A newer sign-in replaced this one, which was still waiting in the
    /// browser. The UI matches this text.
    #[error("Sign-in cancelled")]
    SignInCancelled,
    /// A Gmail call failed.
    #[error("{0}")]
    Gmail(#[from] ub_gmail::GmailError),
    /// The configured Gmail endpoint is not one.
    #[error(transparent)]
    GmailEndpoint(#[from] ub_gmail::InvalidBaseUrl),
    /// The HTTP client could not be built.
    #[error(transparent)]
    Http(#[from] ub_gmail::HttpClientError),
    /// A file of the app's data folder could not be written.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}
