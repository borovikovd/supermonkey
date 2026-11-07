//! The crate's tests that the end-to-end suite can't observe (e2e/COVERAGE.md,
//! kept unit tests): scheduling, OAuth, and unsubscribe safety, against the fake.

#![expect(
    clippy::unwrap_used,
    reason = "Test helpers outside #[test] fns fail loudly on purpose."
)]

mod client;
mod common;
mod oauth;
mod scheduler;
