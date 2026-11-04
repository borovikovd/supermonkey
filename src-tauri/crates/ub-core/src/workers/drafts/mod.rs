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

use ub_gmail::{GmailError, Outcome};

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
    /// The database refused a read or write.
    #[error(transparent)]
    Store(#[from] Error),
}

impl WriteFailure {
    /// Gmail's refusal, as its toast says it, when the write can never go
    /// through as it is.
    pub(crate) fn refusal(&self) -> Option<&str> {
        match self {
            Self::Gmail(e) => match e.outcome() {
                Outcome::Rejected { message, .. } => Some(message),
                _ => None,
            },
            Self::Store(_) => None,
        }
    }
}

/// A failure that is not Gmail's (a file gone, a draft id that can't be a
/// header), surfaced as a rejection so the write rolls back instead of
/// retrying forever.
pub(crate) fn local_rejection(message: impl Into<String>) -> GmailError {
    GmailError::new(Outcome::Rejected {
        status: 0,
        message: message.into(),
        reason: None,
    })
}
