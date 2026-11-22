#![expect(
    clippy::unwrap_used,
    reason = "Test helpers outside #[test] fns fail loudly on purpose."
)]

mod client;
mod common;
mod oauth;
mod scheduler;
