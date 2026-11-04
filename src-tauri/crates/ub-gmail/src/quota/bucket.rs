//! The token bucket of quota units, and the background spending window.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

use super::Units;

/// Rounding slack when comparing a refilled level with a target.
const EPSILON: f64 = 1e-6;

/// How long the background spending cap looks back.
pub(super) const WINDOW: Duration = Duration::from_secs(60);

/// A bucket of quota units that starts full and refills continuously. Every
/// method takes the time, so the bucket itself never reads a clock.
#[derive(Debug)]
pub(super) struct Bucket {
    capacity: f64,
    refill_per_sec: f64,
    level: f64,
    updated: Instant,
}

impl Bucket {
    /// A full bucket.
    pub(super) fn new(capacity: Units, refill_per_minute: Units, now: Instant) -> Self {
        Self {
            capacity: f64::from(capacity.0),
            refill_per_sec: f64::from(refill_per_minute.0) / 60.0,
            level: f64::from(capacity.0),
            updated: now,
        }
    }

    /// Units held at `now`.
    pub(super) fn level(&mut self, now: Instant) -> f64 {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.level = elapsed
            .mul_add(self.refill_per_sec, self.level)
            .min(self.capacity);
        self.updated = now;
        self.level
    }

    /// Whether the bucket holds at least `units` at `now`.
    pub(super) fn has(&mut self, units: f64, now: Instant) -> bool {
        self.level(now) + EPSILON >= units
    }

    /// Spends `units`. The level may go below zero; the debt refills like any
    /// other deficit.
    pub(super) fn take(&mut self, units: Units, now: Instant) {
        self.level(now);
        self.level -= f64::from(units.0);
    }

    /// Returns units taken for a request that never ran.
    pub(super) fn give_back(&mut self, units: Units, now: Instant) {
        self.level(now);
        self.level = (self.level + f64::from(units.0)).min(self.capacity);
    }

    /// How long from `now` until the bucket holds `units`. A bucket that
    /// never refills answers a day, so the caller still wakes to look again.
    pub(super) fn time_until(&mut self, units: f64, now: Instant) -> Duration {
        let missing = (units - self.level(now)).max(0.0);
        Duration::try_from_secs_f64(missing / self.refill_per_sec)
            .unwrap_or(Duration::from_hours(24))
    }
}

/// What priorities 4 to 6 spent in the last [`WINDOW`].
#[derive(Debug, Default)]
pub(super) struct BackgroundWindow {
    spends: VecDeque<Spend>,
}

/// One background request's spending, numbered so a request that never ran
/// can take back exactly its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Spend {
    pub(super) at: Instant,
    pub(super) units: Units,
    pub(super) ticket: u64,
}

impl BackgroundWindow {
    /// Units spent in the window ending at `now`, dropping older spends.
    pub(super) fn spent(&mut self, now: Instant) -> u64 {
        while self
            .spends
            .front()
            .is_some_and(|s| now.saturating_duration_since(s.at) >= WINDOW)
        {
            self.spends.pop_front();
        }
        self.spends.iter().map(|s| u64::from(s.units.0)).sum()
    }

    /// When the oldest spend in the window leaves it.
    pub(super) fn next_expiry(&self) -> Option<Instant> {
        self.spends.front().map(|s| s.at + WINDOW)
    }

    /// Records a spend.
    pub(super) fn record(&mut self, spend: Spend) {
        self.spends.push_back(spend);
    }

    /// Takes back the spend numbered `ticket`, if it is still in the window.
    pub(super) fn cancel(&mut self, ticket: u64) {
        if let Some(i) = self.spends.iter().rposition(|s| s.ticket == ticket) {
            self.spends.remove(i);
        }
    }
}
