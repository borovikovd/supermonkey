use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;

use super::{Activity, Busy, parked};
use crate::clock::Clock;

pub const DEFERRED_KICK: &str = "kick-deferred";

#[derive(Debug)]
pub(crate) struct Kick {
    activity: Arc<Activity>,
    notify: Notify,
    pending: Mutex<Option<Busy>>,
    waiting: AtomicUsize,
}

impl Kick {
    pub(crate) fn new(activity: Arc<Activity>) -> Self {
        Self {
            activity,
            notify: Notify::new(),
            pending: Mutex::new(None),
            waiting: AtomicUsize::new(0),
        }
    }

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

    pub(crate) async fn wait(&self, clock: &dyn Clock, timeout: Duration) {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let _waiting = Waiting(&self.waiting);
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

struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
