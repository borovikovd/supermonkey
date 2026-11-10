//! The op queue's contents (SPEC Offline, Gmail errors): what each queued
//! Gmail mutation does, how an action queues it and a rejection rolls it
//! back, and how the changes still on their way to Gmail stay on top of
//! what Gmail says (local changes win).
//!
//! The account worker's runner (`workers::ops`) sends them; everything here
//! runs on the writer thread or is plain data.

mod landed;
mod local;
mod payload;
mod revert;

pub(crate) use landed::{Landed, ReadTicket};
pub(crate) use local::LocalChanges;
pub(crate) use payload::{LabelCreateOp, ModifyOp};
pub(crate) use revert::{OpOrigin, queue_modify, queue_revert, reject, relabel, revert};
