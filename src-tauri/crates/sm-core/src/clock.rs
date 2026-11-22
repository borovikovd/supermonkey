use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use sm_domain::Millis;
use tokio::sync::Notify;

use crate::activity::{Activity, Busy, parked};

pub trait Clock: Send + Sync {
    fn now(&self) -> Millis;

    fn timer(&self, duration: Duration) -> Pin<Box<dyn Future<Output = Option<Busy>> + Send + '_>>;
}

pub(crate) async fn sleep(clock: &dyn Clock, duration: Duration) {
    let _fired = parked(clock.timer(duration)).await;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Millis {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Millis(i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX))
    }

    fn timer(&self, duration: Duration) -> Pin<Box<dyn Future<Output = Option<Busy>> + Send + '_>> {
        Box::pin(async move {
            tokio::time::sleep(duration).await;
            None
        })
    }
}

#[derive(Debug)]
pub struct ManualClock {
    activity: Arc<Activity>,
    state: Mutex<ManualState>,
}

#[derive(Debug)]
struct ManualState {
    wall: Millis,
    mono: u64,
    next_id: u64,
    sleepers: BTreeMap<(u64, u64), Arc<Sleeper>>,
}

#[derive(Debug, Default)]
struct Sleeper {
    fired: Mutex<Option<Busy>>,
    notify: Notify,
}

impl ManualClock {
    pub const fn new(activity: Arc<Activity>, wall: Millis) -> Self {
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

    pub fn next_due_within(&self, ms: u64) -> Option<u64> {
        let s = self.state.lock();
        let (&(deadline, _), _) = s.sleepers.first_key_value()?;
        let wait = deadline.saturating_sub(s.mono);
        (wait <= ms).then_some(wait)
    }

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

    fn timer(&self, duration: Duration) -> Pin<Box<dyn Future<Output = Option<Busy>> + Send + '_>> {
        Box::pin(async move { Some(self.sleep(duration).await) })
    }
}

struct Registered<'a> {
    clock: &'a ManualClock,
    key: (u64, u64),
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.clock.state.lock().sleepers.remove(&self.key);
    }
}
