//! One account's worker (ARCHITECTURE.md, sections 4 and 8.2): a supervised
//! task group under its own cancellation token. Its tasks:
//!
//! - the poller: the connect sync, then `history.list` every 45 s, on
//!   focus, and when kicked, with the full resync when history expired;
//! - the fetchers: the fetch queue's metadata and body reads, in priority
//!   order, and the contacts backfill;
//! - the ops runner (`workers::ops`): queued Gmail mutations, in order;
//! - the outbox runner (`workers::outbox`): sends, once their undo window
//!   passes;
//! - one-off reads: a thread the user opens, a label refresh, a Gmail
//!   listing for a list subscription.
//!
//! They share an [`AccountCtx`] and talk through its kicks, its queue, and
//! the database. Every Gmail result is parsed and sanitized here and written
//! as a small sync job (`ingest`). Stopping cancels the group and waits for
//! it; a slow or failing account never holds up another.

mod contacts;
mod ctx;
mod fetcher;
mod freshness;
mod health;
mod history;
mod ingest;
mod poller;
mod profile;
mod queue;
mod read;
mod resync;
mod worker;

pub(crate) use ctx::{AccountCtx, OFFLINE_RETRY, SignOutSender, WorkerDeps, WorkerId};
pub(crate) use fetcher::{fetch_thread, is_current, needs_bodies};
pub(crate) use freshness::Freshness;
pub(crate) use ingest::store_draft_message;
pub(crate) use queue::OTHER_META;
pub(crate) use queue::{FetchQueue, LATE, Work};
pub(crate) use read::{ReadMessage, draft_message, parse};
pub(crate) use worker::{AccountWorker, WorkerParts};
