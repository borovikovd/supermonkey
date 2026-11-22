use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub(crate) struct Runs<K>(Mutex<Table<K>>);

#[derive(Debug)]
struct Table<K> {
    running: HashMap<K, Running>,
    last: u64,
}

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
    pub(crate) fn begin(self: &Arc<Self>, key: K) -> Option<Run<K>> {
        let mut table = self.0.lock();
        if table.running.contains_key(&key) {
            return None;
        }
        let (run, _) = self.insert(&mut table, key);
        Some(run)
    }

    pub(crate) fn replace(self: &Arc<Self>, key: K) -> Run<K> {
        let mut table = self.0.lock();
        let (run, replaced) = self.insert(&mut table, key);
        drop(table);
        if let Some(replaced) = replaced {
            replaced.stop.cancel();
        }
        run
    }

    pub(crate) fn stop(&self, key: &K) {
        let running = self.0.lock().running.remove(key);
        if let Some(running) = running {
            running.stop.cancel();
        }
    }

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

#[derive(Debug)]
pub(crate) struct Run<K: Eq + Hash> {
    runs: Arc<Runs<K>>,
    key: K,
    number: u64,
    stop: CancellationToken,
}

impl<K: Eq + Hash> Run<K> {
    pub(crate) const fn stopped(&self) -> &CancellationToken {
        &self.stop
    }

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
