//! App time (ARCHITECTURE.md, section 8.3): the system clock in the app, a
//! manual one the E2E server moves. Every time check and every sleep in the
//! core goes through a [`Clock`].

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use sm_domain::Millis;
use tokio::sync::Notify;

use crate::activity::{Activity, Busy, parked};

/// A timer from [`Clock::timer`]. It resolves with the activity of its
/// firing, on a clock that has one, for the caller to drop once resumed.
pub type Timer<'a> = Pin<Box<dyn Future<Output = Option<Busy>> + Send + 'a>>;

/// The core's time: the wall clock and the timers that run on it.
pub trait Clock: Send + Sync {
    /// App time.
    fn now(&self) -> Millis;

    /// Resolves after `duration` of app time. Prefer [`sleep`], which also
    /// parks the task while it waits.
    fn timer(&self, duration: Duration) -> Timer<'_>;
}

/// Waits `duration` of app time. The task is not busy while it waits.
pub async fn sleep(clock: &dyn Clock, duration: Duration) {
    // Held until the task has resumed, so its activity never drops to zero between.
    let _fired = parked(clock.timer(duration)).await;
}

/// The system clock and tokio's timers: the app's time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Millis {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Millis(i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX))
    }

    fn timer(&self, duration: Duration) -> Timer<'_> {
        Box::pin(async move {
            tokio::time::sleep(duration).await;
            None
        })
    }
}

/// A clock that moves only when told to, for the E2E server and tests. App
/// time (the wall clock) and the time timers count against move together,
/// except in [`ManualClock::jump`], which is the Mac asleep: the wall clock
/// moves and no timer notices.
pub struct ManualClock {
    activity: Arc<Activity>,
    state: Mutex<ManualState>,
}

struct ManualState {
    /// App time.
    wall: Millis,
    /// Elapsed time for timers, ms.
    mono: u64,
    next_id: u64,
    /// By deadline, then by start order.
    sleepers: BTreeMap<(u64, u64), Arc<Sleeper>>,
}

#[derive(Default)]
struct Sleeper {
    /// Set when the timer fires; taken by the sleeping task.
    fired: Mutex<Option<Busy>>,
    notify: Notify,
}

impl ManualClock {
    /// A clock standing at `wall`, whose fired timers count in `activity`.
    pub fn new(activity: Arc<Activity>, wall: Millis) -> Self {
        Self {
            activity,
            state: Mutex::new(ManualState {
                wall,
                mono: 0,
                next_id: 0,
                sleepers: BTreeMap::new(),
            }),
        }
    }

    /// Moves time forward by `ms` and fires every timer due by then, each
    /// once, as Playwright's `clock.fastForward` does. Returns how many fired.
    pub fn advance(&self, ms: u64) -> usize {
        let due = {
            let mut s = self.state.lock();
            let target = s.mono.saturating_add(ms);
            s.wall = s.wall.plus(i64::try_from(ms).unwrap_or(i64::MAX));
            s.mono = target;
            let later = s.sleepers.split_off(&(target.saturating_add(1), 0));
            std::mem::replace(&mut s.sleepers, later)
        };
        for sleeper in due.values() {
            let fired = self.activity.busy("timer");
            *sleeper.fired.lock() = Some(fired);
            sleeper.notify.notify_one();
        }
        due.len()
    }

    /// How long until the next timer is due, if one is due within `ms`.
    pub fn next_due_within(&self, ms: u64) -> Option<u64> {
        let s = self.state.lock();
        let (&(deadline, _), _) = s.sleepers.first_key_value()?;
        let wait = deadline.saturating_sub(s.mono);
        (wait <= ms).then_some(wait)
    }

    /// Moves the wall clock without firing anything: the Mac was asleep for
    /// `ms`. Timers resume where they were, as monotonic timers do.
    pub fn jump(&self, ms: u64) {
        let mut s = self.state.lock();
        s.wall = s.wall.plus(i64::try_from(ms).unwrap_or(i64::MAX));
    }

    async fn sleep(&self, duration: Duration) -> Busy {
        let ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        if ms == 0 {
            return self.activity.busy("timer");
        }
        let sleeper = Arc::new(Sleeper::default());
        let key = {
            let mut s = self.state.lock();
            let id = s.next_id;
            s.next_id = s.next_id.saturating_add(1);
            let key = (s.mono.saturating_add(ms), id);
            s.sleepers.insert(key, Arc::clone(&sleeper));
            key
        };
        let _registered = Registered { clock: self, key };
        loop {
            // `advance` stores a permit, so a firing between the check and
            // the wait is not lost.
            let notified = sleeper.notify.notified();
            let fired = sleeper.fired.lock().take();
            if let Some(fired) = fired {
                return fired;
            }
            notified.await;
        }
    }

    #[cfg(test)]
    pub(crate) fn sleepers(&self) -> usize {
        self.state.lock().sleepers.len()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Millis {
        self.state.lock().wall
    }

    fn timer(&self, duration: Duration) -> Timer<'_> {
        Box::pin(async move { Some(self.sleep(duration).await) })
    }
}

/// Removes a timer from the clock when it ends or is dropped.
struct Registered<'a> {
    clock: &'a ManualClock,
    key: (u64, u64),
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.clock.state.lock().sleepers.remove(&self.key);
    }
}
