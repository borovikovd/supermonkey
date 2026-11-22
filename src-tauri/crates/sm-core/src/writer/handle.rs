use std::path::Path;
use std::sync::Arc;

use sm_store::{Pending, Priority, StoreError, WriteTx, Writer};

use super::{WriterState, on_commit, on_rollback};
use crate::activity::Activity;
use crate::clock::Clock;
use crate::effects::EffectSender;

#[derive(Clone)]
pub(crate) struct CoreWriter {
    writer: Writer<WriterState>,
    activity: Arc<Activity>,
}

impl CoreWriter {
    pub(crate) fn start(
        path: &Path,
        activity: Arc<Activity>,
        router: EffectSender,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, StoreError> {
        let state = WriterState::new(Arc::clone(&activity), router, clock);
        let writer = Writer::start(path, state, on_commit, on_rollback)?;
        Ok(Self { writer, activity })
    }

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

    pub(crate) fn queue<F>(&self, priority: Priority, what: &'static str, job: F)
    where
        F: FnOnce(&mut WriteTx<'_>, &mut WriterState) -> Result<(), StoreError> + Send + 'static,
    {
        drop(self.call(priority, move |tx, state| {
            job(tx, state).inspect_err(|e| tracing::warn!(error = %e, "couldn't {what}"))
        }));
    }

    pub(crate) fn stop(&self) -> Pending<(), StoreError> {
        self.writer.stop()
    }
}
