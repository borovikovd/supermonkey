//! A wake-up for every task waiting at once: the network came back.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;

use super::{Activity, Busy, parked};
use crate::clock::Clock;

/// Wakes every task waiting in [`Signal::wait`] when it fires; a task that
/// starts waiting later waits for the next firing. Unlike a
/// [`Kick`](super::Kick), it keeps nothing for later. Each task it wakes counts as
/// busy until it has resumed, so the core never looks idle between the
/// firing and the work it starts.
pub struct Signal {
    activity: Arc<Activity>,
    state: Mutex<Waiters>,
}

#[derive(Default)]
struct Waiters {
    next_id: u64,
    waiting: BTreeMap<u64, Arc<Waiter>>,
}

#[derive(Default)]
struct Waiter {
    /// Set when the signal fires; taken by the waiting task.
    fired: Mutex<Option<Busy>>,
    notify: Notify,
}

impl Signal {
    /// A signal whose wake-ups count in `activity`.
    pub fn new(activity: Arc<Activity>) -> Self {
        Self {
            activity,
            state: Mutex::new(Waiters::default()),
        }
    }

    /// Wakes every task waiting now.
    pub fn fire(&self) {
        let woken: Vec<Arc<Waiter>> = {
            let mut state = self.state.lock();
            std::mem::take(&mut state.waiting).into_values().collect()
        };
        for waiter in woken {
            *waiter.fired.lock() = Some(self.activity.busy("signal"));
            waiter.notify.notify_one();
        }
    }

    /// Waits until the signal fires, or for `timeout` of app time on `clock`.
    /// The task is not busy while it waits.
    pub async fn wait(&self, clock: &dyn Clock, timeout: Duration) {
        self.wait_unless(clock, timeout, || false).await;
    }

    /// Waits as [`Signal::wait`] does, unless `ready` holds once the wait is
    /// in place: a firing that makes it hold is never missed in between.
    pub async fn wait_unless(
        &self,
        clock: &dyn Clock,
        timeout: Duration,
        ready: impl FnOnce() -> bool,
    ) {
        let waiter = Arc::new(Waiter::default());
        let id = {
            let mut state = self.state.lock();
            let id = state.next_id;
            state.next_id = state.next_id.saturating_add(1);
            state.waiting.insert(id, Arc::clone(&waiter));
            id
        };
        let _registered = Registered { signal: self, id };
        if ready() {
            return;
        }
        // Held until the task has resumed, so activity never drops to zero between.
        let _woken = parked(async {
            tokio::select! {
                woken = fired(&waiter) => Some(woken),
                timer = clock.timer(timeout) => timer,
            }
        })
        .await;
    }
}

/// Resolves once `waiter` has fired, with the firing's activity.
async fn fired(waiter: &Waiter) -> Busy {
    loop {
        // `fire` stores a permit, so a firing between the check and the wait
        // is not lost.
        let notified = waiter.notify.notified();
        let fired = waiter.fired.lock().take();
        if let Some(busy) = fired {
            return busy;
        }
        notified.await;
    }
}

/// Removes a waiter from its signal when its wait ends or is dropped.
struct Registered<'a> {
    signal: &'a Signal,
    id: u64,
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.signal.state.lock().waiting.remove(&self.id);
    }
}
