//! Back-off for rate limits, 5xx, and no response (SPEC Gmail errors): 1 s
//! doubling to 60 s with jitter, or `Retry-After` when Google gives it.
//!
//! Whether to retry is the caller's call: the core's queue and outbox keep an
//! action pending across retries, and `drafts.create` and `drafts.send` must
//! not simply be repeated after no clear answer. This module only says how
//! long to wait, so the core sleeps on its own clock.

use std::time::Duration;

use rand::Rng as _;

use crate::outcome::GmailError;

const FIRST: Duration = Duration::from_secs(1);
const MAX: Duration = Duration::from_secs(60);

/// The wait before retry number `attempt` (0 for the first): 1 s doubling to
/// 60 s, each with ±20% jitter and never above 60 s. A `Retry-After` wins.
#[must_use]
fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(wait) = retry_after {
        return wait;
    }
    let base = FIRST.saturating_mul(1 << attempt.min(16)).min(MAX);
    base.mul_f64(rand::rng().random_range(0.8..1.2)).min(MAX)
}

/// One call's back-off state: how many times it has failed in a row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    /// No failures yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts a failure and says how long to wait before the next try.
    pub fn next_delay(&mut self, err: &GmailError) -> Duration {
        let wait = backoff(self.failures, err.retry_after());
        self.failures = self.failures.saturating_add(1);
        wait
    }

    /// Starts over after a success.
    pub const fn reset(&mut self) {
        self.failures = 0;
    }
}
