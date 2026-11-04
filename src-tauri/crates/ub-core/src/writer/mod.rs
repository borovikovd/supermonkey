//! The core on the writer thread (ARCHITECTURE.md, sections 4 and 5): its
//! state, and what each commit sends. Every job, whatever its priority,
//! goes through [`CoreWriter::call`].

mod commit;
mod handle;
mod state;

pub(crate) use commit::{on_commit, on_rollback};
pub(crate) use handle::CoreWriter;
pub use state::ChannelSink;
pub(crate) use state::WriterState;
