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
}

impl Network {
    pub(crate) fn new(writer: CoreWriter, activity: Arc<Activity>) -> Self {
        Self {
            online: Arc::new(AtomicBool::new(true)),
            writer,
            back: Signal::new(activity),
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
}
