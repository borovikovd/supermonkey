//! The store through its public API: the writer, the derived thread rows,
//! routing, ids, search, drafts, and the read pool.

#![expect(
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    reason = "test helpers fail loudly on purpose"
)]

mod common;
mod drafts;
mod ids;
mod routing;
mod search;
mod threads;
mod views;
mod writer;
