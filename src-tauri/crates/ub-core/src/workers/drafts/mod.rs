//! Drafts on the Gmail side (SPEC Drafts, Gmail errors): writing a draft
//! with `drafts.create` or `drafts.update` (`write`), the queued upsert and
//! delete (`ops`), reading Gmail's drafts back (`reconcile`), and
//! downloading a read draft's parts (`parts`).
//!
//! `drafts.create` is unsafe to repeat: every draft this app writes carries
//! `X-Ubermensch-Draft` with its local id, which is how a create with no
//! clear answer is recovered, and how sync tells this app's drafts apart.

mod failure;
mod ops;
mod parts;
mod reconcile;
mod write;

pub(crate) use failure::WriteFailure;
pub(crate) use ops::{Upserted, delete_copies, delete_draft, upsert};
pub(crate) use parts::download_parts;
pub(crate) use reconcile::{Occasion, read_skipped, reconcile, stops_the_account};
pub(crate) use write::{load, write};
