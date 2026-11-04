//! The async workers (ARCHITECTURE.md, sections 4 and 8.2): one worker per
//! signed-in account (`account`) with its ops runner (`ops`), outbox runner
//! (`outbox`), and draft sync (`drafts`); the registry that starts and stops
//! them (`registry`); Gmail listings for list subscriptions (`search`); and
//! the app's network flag (`network`); and unsubscribe and block
//! (`unwanted`). Effects reach them through [`crate::effects`]'s router.
//!
//! The other modules are extension points that later slices of the rewrite
//! fill in.

mod account;
mod attachments;
mod drafts;
mod files;
mod network;
mod notifications;
mod ops;
mod outbox;
mod registry;
mod search;
mod snooze;
pub(crate) mod unwanted;

pub(crate) use account::{AccountWorker, ReadMessage, WorkerDeps, store_draft_message};
pub(crate) use attachments::open_attachment;
pub(crate) use files::safe_name;
pub(crate) use network::Network;
pub(crate) use registry::AccountWorkers;
pub(crate) use search::Searches;
