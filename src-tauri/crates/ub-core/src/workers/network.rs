//! Whether Gmail is reachable (SPEC Offline, Gmail errors): one flag for the
//! app, set by the outcome of every Gmail call and shown by the app
//! subscription.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ub_store::Priority;

use crate::activity::{Activity, Signal};
use crate::clock::Clock;
use crate::writer::CoreWriter;

/// The app's view of the network, and the wake-up for every task waiting
/// for it.
pub(crate) struct Network {
    online: Arc<AtomicBool>,
    writer: CoreWriter,
    back: Signal,
    /// Fires when the network goes away, so the pollers start checking for
    /// it soon instead of at their next interval.
    lost: Signal,
}

impl Network {
    pub(crate) fn new(writer: CoreWriter, activity: Arc<Activity>) -> Self {
        Self {
            online: Arc::new(AtomicBool::new(true)),
            writer,
            back: Signal::new(Arc::clone(&activity)),
            lost: Signal::new(activity),
        }
    }

    /// Whether the last Gmail call reached Gmail.
    pub(crate) fn is_online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }

    /// Records whether a Gmail call reached Gmail. Coming back wakes every
    /// task waiting for the network, in every account.
    pub(crate) fn set_online(&self, online: bool) {
        if self.online.swap(online, Ordering::SeqCst) == online {
            return;
        }
        tracing::info!(online, "Gmail reachability changed");
        let flag = Arc::clone(&self.online);
        // The job reads the flag when it runs, so of two racing changes the
        // writer ends on the last one.
        self.writer
            .queue(Priority::Ui, "record the network state", move |_, state| {
                state.set_online(flag.load(Ordering::SeqCst));
                Ok(())
            });
        if online {
            self.back.fire();
        } else {
            self.lost.fire();
        }
    }

    /// Wakes every task waiting for the network or backing off: the network
    /// changed, so it may be back.
    pub(crate) fn retry_now(&self) {
        self.back.fire();
    }

    /// Waits until the network may be back, or `timeout` of app time.
    pub(crate) async fn wait(&self, clock: &dyn Clock, timeout: Duration) {
        self.back.wait(clock, timeout).await;
    }

    /// Waits until the network goes away, or `timeout` of app time.
    pub(crate) async fn until_lost(&self, clock: &dyn Clock, timeout: Duration) {
        self.lost
            .wait_unless(clock, timeout, || !self.is_online())
            .await;
    }

    /// Waits until a Gmail call reaches Gmail again, which another task's
    /// call reports (the poller retries every `retry` while offline).
    pub(crate) async fn until_online(&self, clock: &dyn Clock, retry: Duration) {
        while !self.is_online() {
            self.back
                .wait_unless(clock, retry, || self.is_online())
                .await;
        }
    }
}
