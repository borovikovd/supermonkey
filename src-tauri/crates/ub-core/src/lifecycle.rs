//! The `Lifecycle` handle: connecting a page, launch, focus, the online
//! flag, and quitting (ARCHITECTURE.md, sections 4 and 8.1).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ub_ipc::{ChannelMessage, LaunchInfo};
use ub_store::{Priority, ReadPool, StoreError};

use crate::actions::{ActionError, Received};
use crate::activity::{Kick, Tasks};
use crate::clock::Clock;
use crate::workers::{AccountWorkers, Network, Notifier, snooze};
use crate::writer::CoreWriter;
use crate::{Error, Notification};

/// How long quitting waits for work the user already did to commit.
pub(crate) const QUIT_DEADLINE: Duration = Duration::from_secs(5);

/// Connecting pages, the online flag, and quitting.
#[derive(Clone)]
pub struct Lifecycle {
    deps: Arc<LifecycleDeps>,
    launched: Arc<AtomicBool>,
    debug: bool,
}

/// What the lifecycle handle starts and stops.
pub(crate) struct LifecycleDeps {
    pub(crate) writer: CoreWriter,
    pub(crate) pool: Arc<ReadPool>,
    pub(crate) received: Arc<Received>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) tasks: Tasks,
    pub(crate) workers: Arc<AccountWorkers>,
    pub(crate) network: Arc<Network>,
    pub(crate) notifier: Arc<Notifier>,
    /// Wakes the snooze timer.
    pub(crate) snoozes: Arc<Kick>,
}

impl Lifecycle {
    pub(crate) fn new(deps: LifecycleDeps, debug: bool) -> Self {
        Self {
            deps: Arc::new(deps),
            launched: Arc::default(),
            debug,
        }
    }

    /// A page's first call: channel messages go to `sink` from now on, in
    /// order after every UI job queued before. Returns where the page's
    /// action numbers continue: after the last one received, so a reload
    /// never reuses a number still queued.
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

    /// Where a page's action numbers continue, and whether this is a debug
    /// build: what [`Lifecycle::connect`] returns, for a page whose channel
    /// was connected apart from its first call (the debug web bridge's event
    /// stream).
    pub fn launch_info(&self) -> LaunchInfo {
        LaunchInfo {
            last_action: self.deps.received.last(),
            debug: self.debug,
        }
    }

    /// Starts the work that talks to Gmail, once, after
    /// [`App::start`](crate::App::start). First, in one job queued ahead of
    /// any page's first load, due snoozes wake (SPEC Snooze: before the list
    /// is shown), the outbox is settled (sends already sent lose their undo
    /// entries, sends that missed their time show `Email delayed`), and the
    /// untouched rule runs for drafts whose first composer never closed.
    /// Then the snooze timer and each signed-in account's worker start
    /// (SPEC Accounts: one with no stored token shows Sign in again). The
    /// E2E server seeds accounts between the two. Only the first call does
    /// anything.
    pub fn launch(&self) {
        if self.launched.swap(true, Ordering::SeqCst) {
            return;
        }
        let deps = &self.deps;
        let now = deps.clock.now();
        let settled = deps.writer.call(Priority::Ui, move |tx, state| {
            crate::snooze::wake_due(tx, state, now)?;
            crate::outbox::on_launch(tx, state, now)?;
            crate::drafts::on_launch(tx, state, now)?;
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

    /// The main window gained or lost focus. Focus polls every account and
    /// reads its `sendAs` and labels again (SPEC Sync). On focus, returns
    /// the notification the focus counts as a click on, if any: the shell
    /// then shows that account's Inbox on its thread.
    pub fn focus(&self, focused: bool) -> Option<Notification> {
        tracing::debug!(focused, "window focus changed");
        if focused {
            self.deps.workers.refresh_all();
        }
        self.deps.notifier.focus(focused)
    }

    /// Records whether Gmail is reachable, as the outcome of a Gmail call
    /// does; the app subscription shows it. Ordered with the UI's actions.
    pub async fn set_online(&self, online: bool) -> Result<(), Error> {
        self.deps.network.set_online(online);
        self.deps
            .writer
            .call(Priority::Ui, |_, _| Ok::<_, StoreError>(()))
            .await?;
        Ok(())
    }

    /// Quits: nothing new starts, the writer commits the UI jobs already
    /// queued (the user's last actions) and drops queued sync work, then
    /// every task ends. Gives up after five seconds (`QUIT_DEADLINE`) and
    /// returns anyway.
    /// The E2E server restarts the core in-process this way.
    pub async fn quit(&self) {
        let deadline = tokio::time::Instant::now() + QUIT_DEADLINE;
        self.deps.tasks.cancel();
        match tokio::time::timeout_at(deadline, self.deps.writer.stop()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "the writer stopped with an error"),
            Err(_) => tracing::warn!("queued UI actions did not commit before the quit deadline"),
        }
        if tokio::time::timeout_at(deadline, self.deps.tasks.stop())
            .await
            .is_err()
        {
            tracing::warn!("tasks were still running at the quit deadline");
        }
    }
}
