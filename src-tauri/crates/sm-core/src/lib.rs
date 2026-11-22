pub mod activity;
pub mod clock;

mod accounts;
mod actions;
mod app;
mod drafts;
mod effects;
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
