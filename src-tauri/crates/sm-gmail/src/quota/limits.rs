//! Quota units and the per-account limits.

use std::fmt;

/// Gmail quota units, as Google's per-method cost table counts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Units(pub u32);

impl fmt::Display for Units {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The budget and concurrency limits of every account. Build-time config in
/// the app; a runtime knob in the E2E server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaConfig {
    /// The bucket's size.
    pub capacity: Units,
    /// Units the bucket regains per minute.
    pub refill_per_minute: Units,
    /// The most priorities 4 to 6 may spend in any 60 s.
    pub background_per_minute: Units,
    /// Priorities 4 to 6 start only while the bucket holds more than this.
    pub background_reserve: Units,
    /// The most requests in flight at once.
    pub max_in_flight: usize,
}

impl QuotaConfig {
    /// SPEC's limits for a project allowed `per_minute` units a minute per
    /// user: the bucket's 1,000 plus a refill of the rest (at least 1,000 a
    /// minute), so no minute spends more than `per_minute`.
    #[must_use]
    pub fn for_budget(per_minute: Units) -> Self {
        let defaults = Self::default();
        Self {
            refill_per_minute: Units(per_minute.0.saturating_sub(defaults.capacity.0).max(1000)),
            ..defaults
        }
    }
}

impl Default for QuotaConfig {
    /// SPEC's limits for the 6,000-unit quota (spending at most about 5,800).
    fn default() -> Self {
        Self {
            capacity: Units(1000),
            refill_per_minute: Units(4800),
            background_per_minute: Units(4000),
            background_reserve: Units(500),
            max_in_flight: 8,
        }
    }
}
