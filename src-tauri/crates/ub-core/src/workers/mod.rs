//! The async workers (ARCHITECTURE.md, sections 4 and 8.2): one worker per
//! signed-in account (`account`) with its ops runner (`ops`), outbox runner
//! (`outbox`), and draft sync (`drafts`); the registry that starts and stops
//! them (`registry`); Gmail listings for list subscriptions (`search`); and
//! the app's network flag (`network`); showing notifications and their
//! click-through (`notifications`); the snooze timer (`snooze`); and
//! unsubscribe and block (`unwanted`). Work stopped from outside, one run per
//! key, goes through `runs`.
//! Effects reach them through [`crate::effects`]'s router.

mod account;
mod attachments;
pub(crate) mod badge;
mod drafts;
mod files;
mod grants;
mod network;
mod notifications;
mod ops;
mod outbox;
mod quarantine;
mod registry;
mod runs;
mod search;
pub(crate) mod snooze;
pub(crate) mod unwanted;

#[cfg(test)]
pub(crate) use account::Freshness;
pub(crate) use account::{AccountWorker, ReadMessage, WorkerDeps, store_draft_message};
pub(crate) use attachments::open_attachment;
pub(crate) use files::safe_name;
pub(crate) use grants::Grant;
pub(crate) use network::Network;
pub(crate) use notifications::Notifier;
pub(crate) use registry::AccountWorkers;
pub(crate) use search::Searches;
