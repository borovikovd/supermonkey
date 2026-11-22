use std::time::Duration;

use rand::Rng as _;

use crate::outcome::GmailError;

const FIRST: Duration = Duration::from_secs(1);
const MAX: Duration = Duration::from_secs(60);

#[must_use]
fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(wait) = retry_after {
        return wait;
    }
    let base = FIRST.saturating_mul(1 << attempt.min(16)).min(MAX);
    base.mul_f64(rand::rng().random_range(0.8..1.2)).min(MAX)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Backoff {
    failures: u32,
}

impl Backoff {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_delay(&mut self, err: &GmailError) -> Duration {
        let wait = backoff(self.failures, err.retry_after());
        self.failures = self.failures.saturating_add(1);
        wait
    }

    pub const fn reset(&mut self) {
        self.failures = 0;
    }
}
