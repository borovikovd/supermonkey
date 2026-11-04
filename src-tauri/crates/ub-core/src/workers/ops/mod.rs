//! The ops runner (SPEC Offline, Gmail errors): one task per account
//! worker that replays the account's queued Gmail mutations in order.
//!
//! An op stays queued until Gmail accepts or rejects it. Offline, it waits
//! for the network; on a transient failure it backs off and retries; on a
//! refused grant the account signs out and the op waits for the next
//! sign-in. Only a rejection rolls it back, with a notice for a toast, and
//! replay goes on with the next op. Each op runs in the account's
//! [`OpsLane`], so replacing the worker never sends one twice.

mod lane;
mod run;
mod runner;

pub(crate) use lane::OpsLane;
pub(crate) use runner::run;
