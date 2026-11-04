//! The `Lifecycle` handle: connecting a page, the online flag, and quitting
//! (ARCHITECTURE.md, sections 4 and 8.1).
//!
//! Extension points: the snooze slice adds to [`Lifecycle::launch`] (wake
//! due snoozes), and the notifications slice to [`Lifecycle::focus`] (a
//! click-through on the last notification).

use std::sync::Arc;
use std::time::Duration;

use ub_ipc::LaunchInfo;
use ub_store::{Priority, StoreError};

use crate::actions::Received;
use crate::activity::Tasks;
use crate::clock::Clock;
use crate::workers::{AccountWorkers, Network};
use crate::writer::{ChannelSink, CoreWriter};
use crate::{Error, Notification};

/// How long quitting waits for work the user already did to commit.
pub const QUIT_DEADLINE: Duration = Duration::from_secs(5);

/// Connecting pages, the online flag, and quitting.
#[derive(Clone)]
pub struct Lifecycle {
    writer: CoreWriter,
    received: Arc<Received>,
    clock: Arc<dyn Clock>,
    tasks: Tasks,
    workers: Arc<AccountWorkers>,
    network: Arc<Network>,
    debug: bool,
}

impl Lifecycle {
    pub(crate) fn new(
        writer: CoreWriter,
        received: Arc<Received>,
        clock: Arc<dyn Clock>,
        tasks: Tasks,
        workers: Arc<AccountWorkers>,
        network: Arc<Network>,
        debug: bool,
    ) -> Self {
        Self {
            writer,
            received,
            clock,
            tasks,
            workers,
            network,
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
        Ok(self.launch_info())
    }

    /// Where a page's action numbers continue, and whether this is a debug
    /// build: what [`Lifecycle::connect`] returns, for a page whose channel
    /// was connected apart from its first call (the debug web bridge's event
    /// stream).
    pub fn launch_info(&self) -> LaunchInfo {
        LaunchInfo {
            last_action: self.received.last(),
            debug: self.debug,
        }
    }

    /// Starts the work that talks to Gmail, once, after
    /// [`App::start`](crate::App::start). First the outbox is settled (sends
    /// already sent lose their undo entries, sends that missed their time
    /// show `Email delayed`) and the untouched rule runs for drafts whose
    /// first composer never closed; then each signed-in account's worker
    /// starts (SPEC Accounts: one with no stored token shows Sign in again).
    /// The E2E server seeds accounts between the two.
    pub fn launch(&self) {
        let (writer, workers) = (self.writer.clone(), Arc::clone(&self.workers));
        let now = self.clock.now();
        self.tasks.spawn("launch", async move {
            let settled = writer
                .call(Priority::Ui, move |tx, state| {
                    crate::outbox::on_launch(tx, state, now)?;
                    crate::drafts::on_launch(tx, state, now)
                })
                .await;
            if let Err(e) = settled {
                tracing::error!(error = %e, "couldn't settle the outbox and drafts at launch");
            }
            workers.launch();
            tracing::info!("launched");
        });
    }

    /// The main window gained or lost focus. Focus polls every account and
    /// reads its `sendAs` and labels again (SPEC Sync). On focus, returns
    /// the notification the focus counts as a click on, if any: the shell
    /// then shows that thread.
    pub fn focus(&self, focused: bool) -> Option<Notification> {
        tracing::debug!(focused, "window focus changed");
        if focused {
            self.workers.refresh_all();
        }
        None
    }

    /// Records whether Gmail is reachable, as the outcome of a Gmail call
    /// does; the app subscription shows it. Ordered with the UI's actions.
    pub async fn set_online(&self, online: bool) -> Result<(), Error> {
        self.network.set_online(online);
        self.writer
            .call(Priority::Ui, |_, _| Ok::<_, StoreError>(()))
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
