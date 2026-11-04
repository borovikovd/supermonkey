//! The `Lifecycle` handle: connecting a page, the online flag, and quitting
//! (ARCHITECTURE.md, sections 4 and 8.1).
//!
//! Extension points: the accounts and sync slices add launch (start each
//! signed-in account's worker, wake due snoozes, settle the outbox, apply
//! the untouched rule to drafts still started fresh), and focus and wake.

use std::sync::Arc;
use std::time::Duration;

use ub_ipc::LaunchInfo;
use ub_store::{Priority, StoreError};

use crate::Error;
use crate::actions::Received;
use crate::activity::Tasks;
use crate::writer::{ChannelSink, CoreWriter};

/// How long quitting waits for work the user already did to commit.
pub const QUIT_DEADLINE: Duration = Duration::from_secs(5);

/// Connecting pages, the online flag, and quitting.
#[derive(Clone)]
pub struct Lifecycle {
    writer: CoreWriter,
    received: Arc<Received>,
    tasks: Tasks,
    debug: bool,
}

impl Lifecycle {
    pub(crate) fn new(
        writer: CoreWriter,
        received: Arc<Received>,
        tasks: Tasks,
        debug: bool,
    ) -> Self {
        Self {
            writer,
            received,
            tasks,
            debug,
        }
    }

    /// A page's first call: channel messages go to `sink` from now on, in
    /// order after every UI job queued before. Returns where the page's
    /// action numbers continue: after the last one received, so a reload
    /// never reuses a number still queued.
    pub async fn connect(&self, sink: ChannelSink) -> Result<LaunchInfo, Error> {
        self.writer
            .call(Priority::Ui, move |_, state| {
                state.set_sink(sink);
                Ok::<_, StoreError>(())
            })
            .await?;
        Ok(LaunchInfo {
            last_action: self.received.last(),
            debug: self.debug,
        })
    }

    /// Records whether Gmail is reachable; the app subscription shows it.
    /// Ordered with the UI's actions.
    pub async fn set_online(&self, online: bool) -> Result<(), Error> {
        self.writer
            .call(Priority::Ui, move |_, state| {
                state.set_online(online);
                Ok::<_, StoreError>(())
            })
            .await?;
        Ok(())
    }

    /// Quits: nothing new starts, the writer commits the UI jobs already
    /// queued (the user's last actions) and drops queued sync work, then
    /// every task ends. Gives up after [`QUIT_DEADLINE`] and returns anyway.
    /// The E2E server restarts the core in-process this way.
    pub async fn quit(&self) {
        let deadline = tokio::time::Instant::now() + QUIT_DEADLINE;
        self.tasks.cancel();
        match tokio::time::timeout_at(deadline, self.writer.stop()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "the writer stopped with an error"),
            Err(_) => tracing::warn!("queued UI actions did not commit before the quit deadline"),
        }
        if tokio::time::timeout_at(deadline, self.tasks.stop())
            .await
            .is_err()
        {
            tracing::warn!("tasks were still running at the quit deadline");
        }
    }
}
