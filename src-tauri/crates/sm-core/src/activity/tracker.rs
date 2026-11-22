use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio::sync::futures::Notified;

#[derive(Debug, Default)]
pub struct Activity {
    busy: Mutex<BTreeMap<&'static str, usize>>,
    changed: Notify,
}

impl Activity {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    pub(crate) fn busy(self: &Arc<Self>, label: &'static str) -> Busy {
        self.add(label);
        Busy {
            activity: Arc::clone(self),
            label,
        }
    }

    pub(crate) fn track<T>(self: &Arc<Self>, label: &'static str, value: T) -> Tracked<T> {
        Tracked {
            value,
            busy: self.busy(label),
        }
    }

    pub fn snapshot(&self) -> BTreeMap<&'static str, usize> {
        self.busy.lock().clone()
    }

    pub fn is_idle(&self) -> bool {
        self.busy.lock().is_empty()
    }

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

#[derive(Debug)]
pub(crate) struct Tracked<T> {
    value: T,
    busy: Busy,
}

impl<T> Tracked<T> {
    pub(crate) const fn get(&self) -> &T {
        &self.value
    }

    pub(crate) fn into_parts(self) -> (T, Busy) {
        (self.value, self.busy)
    }
}
