//! The application (ARCHITECTURE.md, section 8): actions, queries,
//! subscriptions and patches, and the per-account workers. Takes `ub_ipc`
//! actions in and gives `ub_ipc` read models and channel messages out.
//!
//! Start a core with [`App::start`], then:
//! - [`Lifecycle::connect`] a page's [`ChannelSink`]: one
//!   [`ub_ipc::ChannelMessage`] per commit goes there, and each batch of
//!   first loads as its own message;
//! - [`Actions::submit`] the page's numbered actions;
//! - read with [`Queries`];
//! - [`Lifecycle::quit`] to stop.
//!
//! Inside, one writer thread applies every write (`writer`), the
//! subscription registry lives on it (`subs`), store records become IPC
//! read models in one place (`views`), and committed effects go to the
//! effect router ([`effects`]).

pub mod activity;
pub mod clock;
pub mod effects;

mod actions;
mod app;
mod error;
mod lifecycle;
mod platform;
mod queries;
mod subs;
mod views;
mod workers;
mod writer;

#[cfg(test)]
mod tests;

pub use actions::Actions;
pub use app::{App, Config, Deps};
pub use error::Error;
pub use lifecycle::{Lifecycle, QUIT_DEADLINE};
pub use platform::{Notification, Platform, PlatformError};
pub use queries::Queries;
pub use writer::ChannelSink;
