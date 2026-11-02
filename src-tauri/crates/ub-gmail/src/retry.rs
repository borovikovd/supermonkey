//! Backoff for rate limits, 5xx and no-response outcomes (SPEC: Gmail errors).
//!
//! Whether to retry is the caller's call: the app's queue and outbox keep actions pending
//! across retries. [`with_retry`] is a shortcut for idempotent reads only.

use std::future::Future;
use std::time::Duration;

use rand::Rng;

use crate::error::GmailError;

const FIRST: Duration = Duration::from_secs(1);
const MAX: Duration = Duration::from_secs(60);

/// The wait before retry number `attempt` (0 for the first retry): 1 s doubling to 60 s with
/// ±20% jitter, capped at 60 s. A server `Retry-After` wins when given.
pub fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(wait) = retry_after {
        return wait;
    }
    let base = FIRST.saturating_mul(1 << attempt.min(16)).min(MAX);
    base.mul_f64(rand::rng().random_range(0.8..1.2)).min(MAX)
}

/// Runs an idempotent read, retrying transient outcomes ([`GmailError::is_transient`]) with
/// [`backoff`], up to `max_attempts` calls in all. Never use it for writes: a no-response
/// `drafts.create` or `drafts.send` must not simply be repeated.
pub async fn with_retry<T, F, Fut>(max_attempts: u32, mut call: F) -> Result<T, GmailError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, GmailError>>,
{
    let mut attempt = 0;
    loop {
        match call().await {
            Err(e) if e.is_transient() && attempt + 1 < max_attempts => {
                tokio::time::sleep(backoff(attempt, e.retry_after())).await;
                attempt += 1;
            }
            result => return result,
        }
    }
}
