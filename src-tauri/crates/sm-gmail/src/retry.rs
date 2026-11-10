//! Back-off for rate limits, 5xx, and no response (SPEC Gmail errors): 1 s
//! doubling to 60 s with jitter, or `Retry-After` when Google gives it.
//!
//! Whether to retry is the caller's call: the core's queue and outbox keep an
//! action pending across retries, and `drafts.create` and `drafts.send` must
//! not simply be repeated after no clear answer. The waits take a `sleep`
//! function, so the core sleeps on its own clock.

use std::time::Duration;

use rand::Rng as _;

use crate::outcome::GmailError;

const FIRST: Duration = Duration::from_secs(1);
const MAX: Duration = Duration::from_secs(60);

/// The wait before retry number `attempt` (0 for the first): 1 s doubling to
/// 60 s, each with ±20% jitter and never above 60 s. A `Retry-After` wins.
#[must_use]
pub fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
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

    /// Failures in a row so far.
    #[must_use]
    pub const fn failures(self) -> u32 {
        self.failures
    }

    /// Starts over after a success.
    pub const fn reset(&mut self) {
        self.failures = 0;
    }
}

/// Runs an idempotent call, retrying transient outcomes
/// ([`GmailError::is_transient`]) after [`backoff`] waits slept with `sleep`,
/// up to `max_attempts` calls in all. Never use it for `drafts.create` or
/// `drafts.send`.
pub async fn with_retry<T, Call, CallFut, Sleep, SleepFut>(
    max_attempts: u32,
    mut sleep: Sleep,
    mut call: Call,
) -> Result<T, GmailError>
where
    Call: FnMut() -> CallFut,
    CallFut: Future<Output = Result<T, GmailError>>,
    Sleep: FnMut(Duration) -> SleepFut,
    SleepFut: Future<Output = ()>,
{
    let mut backoff = Backoff::new();
    loop {
        match call().await {
            Err(err) if err.is_transient() && backoff.failures() + 1 < max_attempts => {
                let wait = backoff.next_delay(&err);
                tracing::debug!(outcome = %err.outcome(), ?wait, "retrying a Gmail call");
                sleep(wait).await;
            }
            result => return result,
        }
    }
}
