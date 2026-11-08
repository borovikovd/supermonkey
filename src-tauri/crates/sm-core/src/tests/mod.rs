//! Tests over the whole core: a started [`App`] on a temporary database.

mod clock;
mod harness;
mod perf;
mod writer_channel;

pub(crate) use harness::{Harness, NEWEST, eventually, seed};
