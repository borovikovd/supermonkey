use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;

use super::{Activity, Busy, parked};
use crate::clock::Clock;

#[derive(Debug)]
pub(crate) struct Signal {
    activity: Arc<Activity>,
    state: Mutex<Waiters>,
}

#[derive(Debug, Default)]
struct Waiters {
    next_id: u64,
    waiting: BTreeMap<u64, Arc<Waiter>>,
}

#[derive(Debug, Default)]
struct Waiter {
    fired: Mutex<Option<Busy>>,
    notify: Notify,
}

impl Signal {
    pub(crate) fn new(activity: Arc<Activity>) -> Self {
        Self {
            activity,
            state: Mutex::new(Waiters::default()),
        }
    }

    pub(crate) fn fire(&self) {
        let woken: Vec<Arc<Waiter>> = {
            let mut state = self.state.lock();
            std::mem::take(&mut state.waiting).into_values().collect()
        };
        for waiter in woken {
            *waiter.fired.lock() = Some(self.activity.busy("signal"));
            waiter.notify.notify_one();
        }
    }

    pub(crate) async fn wait(&self, clock: &dyn Clock, timeout: Duration) {
        self.wait_unless(clock, timeout, || false).await;
    }

    pub(crate) async fn wait_unless(
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
        let _woken = parked(async {
            tokio::select! {
                woken = fired(&waiter) => Some(woken),
                timer = clock.timer(timeout) => timer,
            }
        })
        .await;
    }
}

async fn fired(waiter: &Waiter) -> Busy {
    loop {
        let notified = waiter.notify.notified();
        let fired = waiter.fired.lock().take();
        if let Some(busy) = fired {
            return busy;
        }
        notified.await;
    }
}

struct Registered<'a> {
    signal: &'a Signal,
    id: u64,
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.signal.state.lock().waiting.remove(&self.id);
    }
}
