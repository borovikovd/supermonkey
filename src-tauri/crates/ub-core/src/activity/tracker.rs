//! The count of work in progress, by kind.

use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio::sync::futures::Notified;

/// Work in progress in one core, by kind. When nothing is busy, the core
/// stays still until time moves or something arrives from outside (a UI
/// action, a Gmail response): the E2E server's `settle()` waits for that.
///
/// One per core, made by whoever starts it and passed in, so two cores in one
/// process (tests) never see each other's work.
#[derive(Debug, Default)]
pub struct Activity {
    /// Work in progress, by kind.
    busy: Mutex<BTreeMap<&'static str, usize>>,
    changed: Notify,
}

impl Activity {
    /// A tracker with nothing busy.
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// Counts one unit of `label` work until the returned guard drops.
    pub fn busy(self: &Arc<Self>, label: &'static str) -> Busy {
        self.add(label);
        Busy {
            activity: Arc::clone(self),
            label,
        }
    }

    /// `value`, busy as `label` until its receiver is done with it.
    pub fn track<T>(self: &Arc<Self>, label: &'static str, value: T) -> Tracked<T> {
        Tracked {
            value,
            busy: self.busy(label),
        }
    }

    /// Work in progress now, by kind. Empty when the core is idle.
    pub fn snapshot(&self) -> BTreeMap<&'static str, usize> {
        self.busy.lock().clone()
    }

    /// Whether nothing is busy.
    pub fn is_idle(&self) -> bool {
        self.busy.lock().is_empty()
    }

    /// Resolves on the next change in activity. Enable it before reading
    /// [`Activity::snapshot`], so a change in between is not missed.
    pub fn changed(&self) -> Notified<'_> {
        self.changed.notified()
    }

    pub(crate) fn add(&self, label: &'static str) {
        *self.busy.lock().entry(label).or_default() += 1;
        self.changed.notify_waiters();
    }

    pub(crate) fn sub(&self, label: &'static str) {
        {
            let mut busy = self.busy.lock();
            if let Some(n) = busy.get_mut(label) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    busy.remove(label);
                }
            }
        }
        self.changed.notify_waiters();
    }
}

/// One unit of work in progress, counted until dropped.
#[must_use = "the work stops counting as soon as this drops"]
#[derive(Debug)]
pub struct Busy {
    activity: Arc<Activity>,
    label: &'static str,
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.activity.sub(self.label);
    }
}

/// A queued value, busy until its receiver is done with it.
#[derive(Debug)]
pub struct Tracked<T> {
    value: T,
    busy: Busy,
}

impl<T> Tracked<T> {
    /// The value.
    pub fn get(&self) -> &T {
        &self.value
    }

    /// The value, and its activity to drop once it is handled.
    pub fn into_parts(self) -> (T, Busy) {
        (self.value, self.busy)
    }
}
