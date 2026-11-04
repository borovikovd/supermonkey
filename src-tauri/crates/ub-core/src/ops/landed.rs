//! Ops that reached Gmail while a read was in flight (SPEC Offline; the
//! stale-answer rule in AGENTS.md).
//!
//! A read's answer may predate an op that landed while it was on its way.
//! Once the op leaves the queue nothing would put its change back on top of
//! that answer, so an archive that landed during a poll would come back
//! undone (a thread back in the Inbox) until the next poll. Reads take a
//! [`ReadTicket`] before they ask Gmail; when their answer is written, the
//! ops that landed since are applied on top of it like queued ones.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use parking_lot::Mutex;

use super::ModifyOp;

/// The account's log of landed ops, kept only while a read that started
/// before them is in flight.
#[derive(Debug, Default)]
pub(crate) struct Landed(Mutex<Log>);

#[derive(Debug, Default)]
struct Log {
    /// The sequence number of the next op to land.
    next: u64,
    /// Landed ops, oldest first, with their sequence numbers.
    ops: VecDeque<(u64, ModifyOp)>,
    /// Reads in flight: how many started at each point of the log.
    reads: BTreeMap<u64, usize>,
}

impl Landed {
    /// Starts a read whose answer will be written. Take it before the
    /// request goes out, and keep it until the answer is written.
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

    /// An op reached Gmail. Record it in the job that takes it off the
    /// queue, so every write sees it either queued or landed.
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

/// A read in flight; see [`Landed`].
#[derive(Debug)]
pub(crate) struct ReadTicket {
    landed: Arc<Landed>,
    at: u64,
}

impl ReadTicket {
    /// The ops that landed since the read started, oldest first.
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
        // Keep only what a read still in flight can need.
        let oldest = log.reads.keys().next().copied().unwrap_or(u64::MAX);
        while log.ops.front().is_some_and(|(seq, _)| *seq < oldest) {
            log.ops.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ub_domain::{GmailMessageId, LabelSet};

    use super::{Landed, ModifyOp};

    fn op(message: &str) -> ModifyOp {
        ModifyOp {
            messages: vec![GmailMessageId::from(message)],
            add: LabelSet::new(),
            remove: ["INBOX"].into_iter().collect(),
            threads: Vec::new(),
        }
    }

    fn names(ops: &[ModifyOp]) -> Vec<&str> {
        ops.iter()
            .flat_map(|o| o.messages.iter().map(GmailMessageId::as_str))
            .collect()
    }

    #[test]
    fn a_read_sees_the_ops_that_landed_after_it_started_and_only_while_in_flight() {
        let landed = Arc::new(Landed::default());
        landed.push(op("before any read"));
        let early = landed.read();
        landed.push(op("m1"));
        let late = landed.read();
        landed.push(op("m2"));

        assert_eq!(names(&early.landed()), ["m1", "m2"]);
        assert_eq!(names(&late.landed()), ["m2"]);

        drop(early);
        assert_eq!(names(&late.landed()), ["m2"], "a later read keeps its ops");
        drop(late);
        landed.push(op("no read in flight"));
        let next = landed.read();
        assert!(
            next.landed().is_empty(),
            "nothing is kept with no read in flight"
        );
        assert!(landed.0.lock().ops.is_empty());
    }
}
