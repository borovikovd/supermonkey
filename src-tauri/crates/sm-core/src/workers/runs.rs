//! Work in flight, at most one run per key, each stoppable from outside: a
//! conversation's body read by thread, a Gmail listing by subscription.
//! Runs are numbered, so one that ends never forgets a newer run of its key.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

/// The runs in flight, by key.
#[derive(Debug)]
pub(crate) struct Runs<K>(Mutex<Table<K>>);

#[derive(Debug)]
struct Table<K> {
    running: HashMap<K, Running>,
    /// The last run's number.
    last: u64,
}

/// One run in flight.
#[derive(Debug)]
struct Running {
    number: u64,
    stop: CancellationToken,
}

impl<K> Default for Runs<K> {
    fn default() -> Self {
        Self(Mutex::new(Table {
            running: HashMap::new(),
            last: 0,
        }))
    }
}

impl<K: Eq + Hash + Clone> Runs<K> {
    /// Starts a run of `key`, unless one is in flight: then `None`.
    pub(crate) fn begin(self: &Arc<Self>, key: K) -> Option<Run<K>> {
        let mut table = self.0.lock();
        if table.running.contains_key(&key) {
            return None;
        }
        let (run, _) = self.insert(&mut table, key);
        Some(run)
    }

    /// Starts a run of `key`, stopping the one in flight.
    pub(crate) fn replace(self: &Arc<Self>, key: K) -> Run<K> {
        let mut table = self.0.lock();
        let (run, replaced) = self.insert(&mut table, key);
        drop(table);
        // Cancelling wakes the replaced run's task: outside the lock.
        if let Some(replaced) = replaced {
            replaced.stop.cancel();
        }
        run
    }

    /// Stops the run of `key`, if one is in flight.
    pub(crate) fn stop(&self, key: &K) {
        let running = self.0.lock().running.remove(key);
        if let Some(running) = running {
            running.stop.cancel();
        }
    }

    /// Records a new run of `key`; returns it, and the run it replaced.
    fn insert(self: &Arc<Self>, table: &mut Table<K>, key: K) -> (Run<K>, Option<Running>) {
        table.last = table.last.wrapping_add(1);
        let (number, stop) = (table.last, CancellationToken::new());
        let replaced = table.running.insert(
            key.clone(),
            Running {
                number,
                stop: stop.clone(),
            },
        );
        let run = Run {
            runs: Arc::clone(self),
            key,
            number,
            stop,
        };
        (run, replaced)
    }
}

/// A run in flight; dropping it ends it.
#[derive(Debug)]
pub(crate) struct Run<K: Eq + Hash> {
    runs: Arc<Runs<K>>,
    key: K,
    number: u64,
    stop: CancellationToken,
}

impl<K: Eq + Hash> Run<K> {
    /// Cancelled once the run is stopped or replaced.
    pub(crate) const fn stopped(&self) -> &CancellationToken {
        &self.stop
    }

    /// Whether it is still its key's run: neither stopped nor replaced.
    pub(crate) fn is_current(&self) -> bool {
        self.runs
            .0
            .lock()
            .running
            .get(&self.key)
            .is_some_and(|r| r.number == self.number)
    }
}

impl<K: Eq + Hash> Drop for Run<K> {
    fn drop(&mut self) {
        let mut table = self.runs.0.lock();
        if table
            .running
            .get(&self.key)
            .is_some_and(|r| r.number == self.number)
        {
            table.running.remove(&self.key);
        }
    }
}
