//! Wake-ups between a core's tasks.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;

use super::{Activity, Busy, parked};
use crate::clock::Clock;

/// The label of a kick whose task was not waiting for it.
pub const DEFERRED_KICK: &str = "kick-deferred";

/// Wakes a task that waits in [`Kick::wait`]. Like `Notify::notify_one`, a
/// kick with no waiter is kept for the next wait. Busy until a waiter takes
/// it: as `kick` when a task was waiting, or as [`DEFERRED_KICK`] when its
/// task was busy elsewhere and takes it at its next wait.
#[derive(Debug)]
pub(crate) struct Kick {
    activity: Arc<Activity>,
    notify: Notify,
    pending: Mutex<Option<Busy>>,
    waiting: AtomicUsize,
}

impl Kick {
    /// A kick counted in `activity`.
    pub(crate) fn new(activity: Arc<Activity>) -> Self {
        Self {
            activity,
            notify: Notify::new(),
            pending: Mutex::new(None),
            waiting: AtomicUsize::new(0),
        }
    }

    /// Wakes the waiter, or the next one to wait.
    pub(crate) fn kick(&self) {
        let label = if self.waiting.load(Ordering::SeqCst) > 0 {
            "kick"
        } else {
            DEFERRED_KICK
        };
        let busy = self.activity.busy(label);
        let extra = {
            let mut pending = self.pending.lock();
            if pending.is_none() {
                *pending = Some(busy);
                None
            } else {
                Some(busy)
            }
        };
        drop(extra);
        self.notify.notify_one();
    }

    /// Waits for a kick, or for `timeout` of app time on `clock`.
    pub(crate) async fn wait(&self, clock: &dyn Clock, timeout: Duration) {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let _waiting = Waiting(&self.waiting);
        // Held until the task has resumed, so activity never drops to zero between.
        let _fired = parked(async {
            tokio::select! {
                () = self.notify.notified() => None,
                fired = clock.timer(timeout) => fired,
            }
        })
        .await;
        let taken = self.pending.lock().take();
        drop(taken);
    }

    #[cfg(test)]
    pub(crate) fn waiters(&self) -> usize {
        self.waiting.load(Ordering::SeqCst)
    }
}

/// Counts a task in [`Kick::wait`] until the wait ends or is dropped.
struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
