//! The per-account quota budget and request scheduling (SPEC Sync).
//!
//! Each account has its own bucket of quota units (1,000, refilling 4,800 a
//! minute, so no minute spends more than about 5,800) and its own eight
//! request slots, so accounts never wait on each other. Requests wait in
//! priority order, 1 first, then in arrival order. Priorities 4 to 6 spend at
//! most 4,000 units in any 60 s, and only while the bucket holds more than 500;
//! priorities 1 to 3 may drain it.
//!
//! Time is tokio's: real monotonic time in the app, and a paused clock that
//! the test moves in tests.

mod bucket;
pub(crate) mod cost;
mod limits;
mod priority;
mod scheduler;

pub use limits::{QuotaConfig, Units};
pub use priority::{InvalidPriority, Priority};
pub use scheduler::{AccountScheduler, Permit, Scheduler};
