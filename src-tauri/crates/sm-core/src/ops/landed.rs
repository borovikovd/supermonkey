use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use parking_lot::Mutex;

use super::ModifyOp;

#[derive(Debug, Default)]
pub(crate) struct Landed(Mutex<Log>);

#[derive(Debug, Default)]
struct Log {
    next: u64,
    ops: VecDeque<(u64, ModifyOp)>,
    reads: BTreeMap<u64, usize>,
}

impl Landed {
    pub(crate) fn read(self: &Arc<Self>) -> ReadTicket {
        let mut log = self.0.lock();
        let at = log.next;
        *log.reads.entry(at).or_default() += 1;
        drop(log);
        ReadTicket {
            landed: Arc::clone(self),
            at,
        }
    }

    pub(crate) fn push(&self, op: ModifyOp) {
        let mut log = self.0.lock();
        if log.reads.is_empty() {
            return;
        }
        let seq = log.next;
        log.next = log.next.saturating_add(1);
        log.ops.push_back((seq, op));
    }
}

#[derive(Debug)]
pub(crate) struct ReadTicket {
    landed: Arc<Landed>,
    at: u64,
}

impl ReadTicket {
    pub(crate) fn landed(&self) -> Vec<ModifyOp> {
        self.landed
            .0
            .lock()
            .ops
            .iter()
            .filter(|(seq, _)| *seq >= self.at)
            .map(|(_, op)| op.clone())
            .collect()
    }
}

impl Drop for ReadTicket {
    fn drop(&mut self) {
        let mut log = self.landed.0.lock();
        if let Some(n) = log.reads.get_mut(&self.at) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                log.reads.remove(&self.at);
            }
        }
        let oldest = log.reads.keys().next().copied().unwrap_or(u64::MAX);
        while log.ops.front().is_some_and(|(seq, _)| *seq < oldest) {
            log.ops.pop_front();
        }
    }
}
