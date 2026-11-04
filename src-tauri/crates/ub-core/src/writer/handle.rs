//! The core's handle to the store's writer thread.

use std::path::Path;
use std::sync::Arc;

use ub_store::{Pending, Priority, StoreError, WriteTx, Writer};

use super::{WriterState, on_commit, on_rollback};
use crate::activity::Activity;
use crate::effects::EffectSender;

/// The one writer (ARCHITECTURE.md, section 6), with the core's state on its
/// thread. Every job counts as busy from when it is queued until its
/// commit's messages and effects are out.
#[derive(Clone)]
pub(crate) struct CoreWriter {
    writer: Writer<WriterState>,
    activity: Arc<Activity>,
}

impl CoreWriter {
    /// Opens the database at `path` and starts the writer thread.
    pub(crate) fn start(
        path: &Path,
        activity: Arc<Activity>,
        router: EffectSender,
    ) -> Result<Self, StoreError> {
        let state = WriterState::new(Arc::clone(&activity), router);
        let writer = Writer::start(path, state, on_commit, on_rollback)?;
        Ok(Self { writer, activity })
    }

    /// Queues a job; see [`Writer::call`]. It runs in its own transaction,
    /// with the writer state.
    pub(crate) fn call<T, E, F>(&self, priority: Priority, job: F) -> Pending<T, E>
    where
        T: Send + 'static,
        E: From<StoreError> + Send + 'static,
        F: FnOnce(&mut WriteTx<'_>, &mut WriterState) -> Result<T, E> + Send + 'static,
    {
        let busy = self.activity.busy("writer");
        self.writer
            .call(priority, move |tx, state: &mut WriterState| {
                state.hold(busy);
                job(tx, state)
            })
    }

    /// Stops the writer after the UI jobs queued so far; see [`Writer::stop`].
    pub(crate) fn stop(&self) -> Pending<(), StoreError> {
        self.writer.stop()
    }
}
