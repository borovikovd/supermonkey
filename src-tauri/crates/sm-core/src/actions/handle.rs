use std::fmt;
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

pub struct Actions {
    writer: CoreWriter,
    received: Arc<Received>,
    clock: Arc<dyn Clock>,
    tasks: Tasks,
}

impl fmt::Debug for Actions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Actions")
            .field("received", &self.received)
            .finish_non_exhaustive()
    }
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

    #[cfg(test)]
    pub(crate) fn last_received(&self) -> ActionNo {
        self.received.last()
    }

    #[cfg(test)]
    pub(crate) async fn drained(&self) -> Result<(), StoreError> {
        self.writer
            .call(Priority::Background, |_, _| Ok::<_, StoreError>(()))
            .await
    }
}
