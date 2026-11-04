//! The application (ARCHITECTURE.md, section 8): actions, queries,
//! subscriptions and patches, and the per-account workers. Takes `ub_ipc`
//! actions in and gives `ub_ipc` read models and channel messages out.
//!
//! Start a core with [`App::start`] and [`Lifecycle::launch`] it, then:
//! - [`Lifecycle::connect`] a page's [`ChannelSink`]: one
//!   [`ub_ipc::ChannelMessage`] per commit goes there, and each batch of
//!   first loads as its own message;
//! - [`Actions::submit`] the page's numbered actions;
//! - read with [`Queries`];
//! - sign in with [`Accounts`], and open links and attachments with [`Mail`];
//! - [`Lifecycle::quit`] to stop.
//!
//! Inside, one writer thread applies every write (`writer`), the
//! subscription registry lives on it (`subs`), store records become IPC
//! read models in one place (`views`), and committed effects go to the
//! effect router ([`effects`]), which hands them to the async workers
//! (`workers`): one task group per signed-in account that syncs it with
//! Gmail, and the Gmail listings of list subscriptions.

pub mod activity;
pub mod clock;
pub mod effects;

mod accounts;
mod actions;
mod app;
mod drafts;
mod error;
mod lifecycle;
mod mail;
mod notify;
mod ops;
mod outbox;
mod platform;
mod queries;
mod read;
mod snooze;
mod subs;
mod views;
mod workers;
mod writer;

#[cfg(test)]
mod tests;

pub use accounts::Accounts;
pub use actions::Actions;
pub use app::{App, Config, Deps, GmailConfig};
pub use error::Error;
pub use lifecycle::Lifecycle;
pub use mail::{MAX_ATTACHMENT_BYTES, Mail, NewAttachment, attachment_too_large};
pub use platform::{Notification, Platform, PlatformError};
pub use queries::Queries;
pub use writer::ChannelSink;
