use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Units(pub u32);

impl fmt::Display for Units {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaConfig {
    pub capacity: Units,
    pub refill_per_minute: Units,
    pub background_per_minute: Units,
    pub background_reserve: Units,
    pub max_in_flight: usize,
}

impl QuotaConfig {
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
