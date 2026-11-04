//! The reads of conversations' bodies in flight (SPEC Sync, priority 2):
//! at most one per thread, each stopped once no subscription shows its
//! thread. Offline, a read waits for the network; without this, every open
//! and prefetch would leave a read of its own waiting, and all of them
//! would go out when the network came back.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use ub_domain::ThreadId;

/// The account's conversation reads in flight, by thread.
#[derive(Debug, Default)]
pub(crate) struct OpenedReads(Mutex<Reads>);

#[derive(Debug, Default)]
struct Reads {
    running: HashMap<ThreadId, Running>,
    /// Numbers the reads, so a read that ended never removes a newer one.
    last: u64,
}

/// One read in flight.
#[derive(Debug)]
struct Running {
    number: u64,
    stop: CancellationToken,
}

impl OpenedReads {
    /// Starts the read of `thread`, unless one is in flight: returns what
    /// it stops on and the claim that ends it, or `None`.
    pub(crate) fn begin(self: &Arc<Self>, thread: ThreadId) -> Option<OpenedRead> {
        let stop = CancellationToken::new();
        let mut reads = self.0.lock();
        if reads.running.contains_key(&thread) {
            return None;
        }
        reads.last = reads.last.wrapping_add(1);
        let number = reads.last;
        reads.running.insert(
            thread,
            Running {
                number,
                stop: stop.clone(),
            },
        );
        drop(reads);
        Some(OpenedRead {
            reads: Arc::clone(self),
            thread,
            number,
            stop,
        })
    }

    /// Stops the read of `thread`, if one is in flight.
    pub(crate) fn stop(&self, thread: ThreadId) {
        let running = self.0.lock().running.remove(&thread);
        if let Some(running) = running {
            running.stop.cancel();
        }
    }
}

/// A read in flight; dropping it ends it.
#[derive(Debug)]
pub(crate) struct OpenedRead {
    reads: Arc<OpenedReads>,
    thread: ThreadId,
    number: u64,
    stop: CancellationToken,
}

impl OpenedRead {
    /// Cancelled once no subscription shows the thread.
    pub(crate) fn stopped(&self) -> &CancellationToken {
        &self.stop
    }
}

impl Drop for OpenedRead {
    fn drop(&mut self) {
        let mut reads = self.reads.0.lock();
        if reads
            .running
            .get(&self.thread)
            .is_some_and(|r| r.number == self.number)
        {
            reads.running.remove(&self.thread);
        }
    }
}
