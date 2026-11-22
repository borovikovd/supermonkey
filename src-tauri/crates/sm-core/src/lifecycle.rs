use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sm_ipc::{ChannelMessage, LaunchInfo};
use sm_store::{Priority, ReadPool, StoreError};

use crate::actions::{ActionError, Received};
use crate::activity::{Kick, Tasks};
use crate::clock::Clock;
use crate::mail::MAX_ATTACHMENT_BYTES;
use crate::workers::{AccountWorkers, Notifier, snooze};
use crate::writer::CoreWriter;
use crate::{Error, Notification};

pub(crate) const QUIT_DEADLINE: Duration = Duration::from_secs(5);

pub struct Lifecycle {
    deps: Arc<LifecycleDeps>,
    launched: AtomicBool,
    debug: bool,
}

impl fmt::Debug for Lifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lifecycle")
            .field("launched", &self.launched)
            .field("debug", &self.debug)
            .finish_non_exhaustive()
    }
}

pub(crate) struct LifecycleDeps {
    pub(crate) writer: CoreWriter,
    pub(crate) pool: Arc<ReadPool>,
    pub(crate) received: Arc<Received>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) tasks: Tasks,
    pub(crate) workers: Arc<AccountWorkers>,
    pub(crate) notifier: Arc<Notifier>,
    pub(crate) snoozes: Arc<Kick>,
}

impl Lifecycle {
    pub(crate) fn new(deps: LifecycleDeps, debug: bool) -> Self {
        Self {
            deps: Arc::new(deps),
            launched: AtomicBool::default(),
            debug,
        }
    }

    pub async fn connect(
        &self,
        sink: Box<dyn FnMut(ChannelMessage) + Send>,
    ) -> Result<LaunchInfo, Error> {
        self.deps
            .writer
            .call(Priority::Ui, move |_, state| {
                state.set_sink(sink);
                Ok::<_, StoreError>(())
            })
            .await?;
        Ok(self.launch_info())
    }

    pub fn launch_info(&self) -> LaunchInfo {
        LaunchInfo {
            last_action: self.deps.received.last(),
            debug: self.debug,
            max_attachment_bytes: u32::try_from(MAX_ATTACHMENT_BYTES).unwrap_or(u32::MAX),
        }
    }

    pub fn launch(&self) {
        if self.launched.swap(true, Ordering::SeqCst) {
            return;
        }
        let deps = &self.deps;
        let now = deps.clock.now();
        let settled = deps.writer.call(Priority::Ui, move |tx, state| {
            crate::snooze::wake_due(tx, state, now)?;
            crate::outbox::on_launch(tx, state, now)?;
            crate::drafts::on_launch(tx, state)?;
            Ok::<_, ActionError>(())
        });
        let launching = Arc::clone(deps);
        deps.tasks.spawn("launch", async move {
            if let Err(e) = settled.await {
                tracing::error!(error = %e, "couldn't wake snoozes and settle the outbox and drafts at launch");
            }
            launching.tasks.spawn(
                "snooze timer",
                snooze::run(
                    Arc::clone(&launching.snoozes),
                    Arc::clone(&launching.pool),
                    launching.writer.clone(),
                    Arc::clone(&launching.clock),
                ),
            );
            launching.workers.launch();
            tracing::info!("launched");
        });
    }

    pub fn focus(&self, focused: bool) -> Option<Notification> {
        tracing::debug!(focused, "window focus changed");
        if focused {
            self.deps.workers.refresh_all();
        }
        self.deps.notifier.focus(focused)
    }

    pub async fn quit(&self) {
        let deadline = tokio::time::Instant::now() + QUIT_DEADLINE;
        self.deps.tasks.cancel();
        self.stop_writer(deadline).await;
        self.stop_tasks(deadline).await;
    }

    async fn stop_writer(&self, deadline: tokio::time::Instant) {
        let Ok(stopped) = tokio::time::timeout_at(deadline, self.deps.writer.stop()).await else {
            tracing::warn!("queued UI actions did not commit before the quit deadline");
            return;
        };
        stopped.unwrap_or_else(|e| tracing::warn!(error = %e, "the writer stopped with an error"));
    }

    async fn stop_tasks(&self, deadline: tokio::time::Instant) {
        if tokio::time::timeout_at(deadline, self.deps.tasks.stop())
            .await
            .is_err()
        {
            tracing::warn!("tasks were still running at the quit deadline");
        }
    }
}
