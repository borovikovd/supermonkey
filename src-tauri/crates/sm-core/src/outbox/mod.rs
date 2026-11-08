//! The outbox on the writer side (SPEC Send and undo): queueing a send in
//! its undo window, undoing it, the state machine its Gmail side moves it
//! through (`machine`), writing each step, and settling it on launch.
//!
//! The runner that talks to Gmail is `workers::outbox`.

mod machine;
mod queue;
mod resolve;

pub(crate) use machine::Event;
pub(crate) use queue::{close_toast, on_launch, send, undo_send};
pub(crate) use resolve::resolve;
