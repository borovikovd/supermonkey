//! UI actions (SPEC UI state, Actions; ARCHITECTURE.md, section 5.1): each
//! numbered action is applied once, in order, on the writer thread. A
//! handler reads and writes through the store's repositories, decides with
//! `sm_domain` rules, and records effects on the writer state.
//!
//! One module per group of actions.

mod accounts;
mod batch;
mod compose;
mod dispatch;
mod error;
mod handle;
mod labels;
mod mail;
mod notices;
mod outbox;
mod rejected;
mod settings;
mod splits;
mod subscriptions;

pub(crate) use dispatch::ActionCtx;
pub(crate) use error::{ActionError, NOT_SAVED, Unsendable};
pub use handle::Actions;
pub(crate) use handle::Received;
pub(crate) use mail::{ThreadsChange, change_threads};
