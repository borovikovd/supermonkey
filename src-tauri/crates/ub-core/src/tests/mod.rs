//! Tests over the whole core: a started [`App`] on a temporary database.

mod accounts;
mod actions;
mod badge;
mod clock;
mod harness;
mod notify;
mod perf;
mod snooze;
mod writer_channel;

pub(crate) use harness::{ACCOUNT_EMAIL, Harness, NEWEST, eventually, message, seed};
