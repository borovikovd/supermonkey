//! Drafts on the Gmail side (SPEC Drafts, Gmail errors): writing a draft
//! with `drafts.create` or `drafts.update` (`write`), the queued upsert and
//! delete (`ops`), reading Gmail's drafts back (`reconcile`), and
//! downloading a read draft's parts (`parts`).
//!
//! `drafts.create` is unsafe to repeat: every draft this app writes carries
//! `X-Ubermensch-Draft` with its local id, which is how a create with no
//! clear answer is recovered, and how sync tells this app's drafts apart.

mod ops;
mod parts;
mod reconcile;
mod write;

use ub_gmail::GmailError;

use crate::Error;

pub(crate) use ops::{Upserted, delete_draft, upsert};
pub(crate) use parts::download_parts;
pub(crate) use reconcile::{Occasion, read_skipped, reconcile};
pub(crate) use write::{load, write};

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
