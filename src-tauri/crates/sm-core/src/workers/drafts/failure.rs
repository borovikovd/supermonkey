use sm_gmail::GmailError;

use crate::Error;

#[derive(Debug, thiserror::Error)]
pub(crate) enum WriteFailure {
    #[error(transparent)]
    Gmail(#[from] GmailError),
    #[error("{0}")]
    Local(String),
    #[error(transparent)]
    Store(#[from] Error),
}
