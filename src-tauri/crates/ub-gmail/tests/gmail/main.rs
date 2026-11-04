//! The crate's tests that the end-to-end suite can't observe (COVERAGE
//! section 9): scheduling, OAuth, and how calls classify, against the fake.

#![expect(
    clippy::unwrap_used,
    reason = "Test helpers outside #[test] fns fail loudly on purpose."
)]

mod client;
mod common;
mod oauth;
mod scheduler;
mod token_store;
