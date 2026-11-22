use sm_store::StoreError;

use crate::PlatformError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("a read task failed")]
    ReadTask(#[from] tokio::task::JoinError),
    #[error("the core needs a tokio runtime")]
    NoRuntime(#[from] tokio::runtime::TryCurrentError),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Tokens(#[from] sm_gmail::TokenStoreError),
    #[error("Sign-in failed: {0}")]
    SignIn(#[from] sm_gmail::SignInError),
    #[error("Sign-in cancelled")]
    SignInCancelled,
    #[error("The draft is gone")]
    DraftGone,
    #[error("{0}")]
    Gmail(#[from] sm_gmail::GmailError),
    #[error(transparent)]
    GmailEndpoint(#[from] sm_gmail::InvalidBaseUrl),
    #[error(transparent)]
    Http(#[from] sm_gmail::HttpClientError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}
