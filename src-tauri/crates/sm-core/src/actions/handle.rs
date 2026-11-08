//! The `Actions` handle: numbered UI actions into writer jobs.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use sm_domain::ActionNo;
use sm_ipc::Submitted;
use sm_store::Priority;
#[cfg(test)]
use sm_store::StoreError;

use super::batch::apply_batch;
use crate::activity::Tasks;
use crate::clock::Clock;
use crate::writer::CoreWriter;

/// The highest action number received, applied or still queued.
#[derive(Debug, Default)]
pub(crate) struct Received(AtomicI64);

impl Received {
    pub(crate) fn note(&self, n: ActionNo) {
        self.0.fetch_max(n.0, Ordering::AcqRel);
    }

    pub(crate) fn last(&self) -> ActionNo {
        ActionNo(self.0.load(Ordering::Acquire))
    }
}

/// Takes the UI's numbered actions (SPEC UI state, Actions).
pub struct Actions {
    writer: CoreWriter,
    received: Arc<Received>,
    clock: Arc<dyn Clock>,
    tasks: Tasks,
}

impl Actions {
    pub(crate) fn new(
        writer: CoreWriter,
        received: Arc<Received>,
        clock: Arc<dyn Clock>,
        tasks: Tasks,
    ) -> Self {
        Self {
            writer,
            received,
            clock,
            tasks,
        }
    }

    /// Queues actions as one writer job at UI priority, ahead of sync work,
    /// and returns as soon as they are queued. The job applies each action
    /// numbered above the last applied one, in order; the commit's channel
    /// message carries the new `ack`.
    ///
    /// If the job's commit fails, every action in it is rejected: a second
    /// job moves `ack` past them, so the mirror drops them and falls back to
    /// Rust's state. An action the page sent that the core couldn't read is
    /// refused alone.
    pub fn submit<A: Into<Submitted>>(&self, actions: Vec<A>) {
        let actions: Vec<Submitted> = actions.into_iter().map(Into::into).collect();
        let Some(last) = actions.iter().map(|a| a.n).max() else {
            return;
        };
        self.received.note(last);
        let now = self.clock.now();
        let applied = self.writer.call(Priority::Ui, move |tx, state| {
            apply_batch(tx, state, actions, now)
        });
        let writer = self.writer.clone();
        self.tasks.spawn("actions", async move {
            let Err(e) = applied.await else {
                return;
            };
            tracing::warn!(error = %e, last = last.0, "an action batch failed to commit; rejecting it");
            let acked = writer
                .call(Priority::Ui, move |tx, _| tx.meta().set_last_action(last))
                .await;
            if let Err(e) = acked {
                tracing::error!(error = %e, last = last.0, "couldn't move ack past a failed batch");
            }
        });
    }

    /// The last action number received, applied or still queued: where a
    /// reloaded page continues, so it never reuses a number still queued.
    pub fn last_received(&self) -> ActionNo {
        self.received.last()
    }

    /// Queues a job that only reads the writer state, for tests to wait on
    /// everything queued before it.
    #[cfg(test)]
    pub(crate) async fn drained(&self) -> Result<(), StoreError> {
        self.writer
            .call(Priority::Background, |_, _| Ok::<_, StoreError>(()))
            .await
    }
}
