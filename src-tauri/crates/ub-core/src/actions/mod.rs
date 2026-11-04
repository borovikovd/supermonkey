//! UI actions (SPEC UI state, Actions; ARCHITECTURE.md, section 5.1): each
//! numbered action is applied once, in order, on the writer thread. A
//! handler reads and writes through the store's repositories, decides with
//! `ub_domain` rules, and records effects on the writer state.
//!
//! One module per group of actions. This slice handles the subscriptions,
//! settings, notices, and send toasts; the rest reject with
//! [`ActionError::NotYetImplemented`] until their slice lands.

mod accounts;
mod batch;
mod compose;
mod dispatch;
mod error;
mod handle;
mod mail;
mod notices;
mod outbox;
mod settings;
mod splits;
mod subscriptions;

pub(crate) use dispatch::ActionCtx;
pub(crate) use error::ActionError;
pub use handle::Actions;
pub(crate) use handle::Received;
