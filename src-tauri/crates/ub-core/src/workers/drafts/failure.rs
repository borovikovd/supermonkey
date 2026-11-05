//! Why a draft write did not reach Gmail.

use ub_gmail::GmailError;

use crate::Error;

/// Why a draft write did not reach Gmail.
#[derive(Debug, thiserror::Error)]
pub(crate) enum WriteFailure {
    /// Gmail's outcome. A rejection (`Outcome::Rejected`) is for good: the
    /// write rolls back. Any other leaves it to run again.
    #[error(transparent)]
    Gmail(#[from] GmailError),
    /// A failure that is not Gmail's but is as final as a rejection: a file
    /// gone, an attachment Gmail no longer has, a message that can't be
    /// built. The write rolls back instead of retrying forever; the text
    /// says why, for the toast.
    #[error("{0}")]
    Local(String),
    /// The database refused a read or write.
    #[error(transparent)]
    Store(#[from] Error),
}
