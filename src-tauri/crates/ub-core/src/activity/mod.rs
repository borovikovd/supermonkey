//! The core's activity: what work is in progress, so the E2E server can tell
//! when the core is idle (`settle()`).
//!
//! Work that can still change state counts as busy: tasks started with
//! [`Tasks::spawn`], queued writer jobs and effects ([`Tracked`]), kicks not
//! yet taken ([`Kick`]), and timers that fired. A task waiting in
//! [`crate::clock::sleep`], [`Kick::wait`], or [`parked`] is not busy until
//! its timer or kick fires.
//!
//! Nothing here is global: the [`Activity`] is made by whoever starts the core
//! and passed to every part that starts work.

mod kick;
mod tasks;
mod tracker;

pub use kick::{DEFERRED_KICK, Kick};
pub use tasks::{Tasks, busy_scope, parked, waiting_on_person};
pub use tracker::{Activity, Busy, Tracked};
