//! An account's fetch queue (SPEC Sync): background reads from Gmail, in
//! priority order and each queued once.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use ub_domain::{GmailThreadId, LabelId};
use ub_gmail::{HistoryId, PageToken, Priority};

use crate::activity::{Activity, Kick};
use crate::clock::Clock;

/// Where a fetch stands in line: its Gmail priority, then a rank that orders
/// the stages SPEC Sync lists within one priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Slot {
    pub(crate) priority: Priority,
    rank: u8,
}

/// Bodies of new messages from a poll or a full resync.
pub(crate) const POLL_BODIES: Slot = Slot {
    priority: Priority::P4,
    rank: 0,
};
/// Inbox metadata, a full resync's Inbox refetch included.
pub(crate) const INBOX_META: Slot = Slot {
    priority: Priority::P4,
    rank: 1,
};
/// Drafts and starred metadata.
pub(crate) const OTHER_META: Slot = Slot {
    priority: Priority::P4,
    rank: 2,
};
/// Inbox bodies.
pub(crate) const INBOX_BODIES: Slot = Slot {
    priority: Priority::P4,
    rank: 3,
};
/// Sent, the bodies of drafts, starred threads, and Gmail search hits, and a
/// full resync's refetch of other threads.
pub(crate) const LATE: Slot = Slot {
    priority: Priority::P5,
    rank: 0,
};
/// The contacts backfill.
pub(crate) const BACKFILL: Slot = Slot {
    priority: Priority::P6,
    rank: 0,
};

/// A `threads.list` the connect sync reads: by label or by query.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Listing {
    /// Threads with this label.
    Label(LabelId),
    /// Threads matching a Gmail query.
    Query(&'static str),
}

/// One background read.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Work {
    /// A `threads.list` page. Each thread it lists gets its metadata at
    /// `meta`, then its bodies at `body`; the next page queues behind it.
    List {
        listing: Listing,
        page: Option<PageToken>,
        meta: Slot,
        body: Option<Slot>,
    },
    /// A thread's metadata, skipped when the cached thread is already at
    /// `history`; then its bodies at `body`.
    Meta {
        thread: GmailThreadId,
        history: Option<HistoryId>,
        body: Option<Slot>,
    },
    /// Every body the thread is missing. Stored bodies are never fetched again.
    Body { thread: GmailThreadId },
    /// A page of the contacts backfill.
    Backfill { page: Option<PageToken> },
}

/// How long an idle fetcher waits before looking at the queue again; a
/// push wakes it sooner.
const IDLE_WAIT: Duration = Duration::from_secs(3600);

/// The queue: a priority heap, with each work item in it at most once.
pub(crate) struct FetchQueue {
    inner: Mutex<Inner>,
    kick: Kick,
}

#[derive(Default)]
struct Inner {
    heap: BinaryHeap<Reverse<(Slot, u64)>>,
    items: HashMap<u64, Work>,
    queued: HashSet<Work>,
    next: u64,
}

impl FetchQueue {
    pub(crate) fn new(activity: Arc<Activity>) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            kick: Kick::new(activity),
        }
    }

    /// Queues `work` at `slot`, unless it is queued already.
    pub(crate) fn push(&self, slot: Slot, work: Work) {
        {
            let mut q = self.inner.lock();
            if q.queued.contains(&work) {
                return;
            }
            q.queued.insert(work.clone());
            let seq = q.next;
            q.next = q.next.saturating_add(1);
            q.items.insert(seq, work);
            q.heap.push(Reverse((slot, seq)));
        }
        self.kick.kick();
    }

    fn pop(&self) -> Option<(Slot, Work)> {
        let mut q = self.inner.lock();
        let Reverse((slot, seq)) = q.heap.pop()?;
        let work = q.items.remove(&seq)?;
        q.queued.remove(&work);
        Some((slot, work))
    }

    /// The next item, first by slot, then by arrival; waits while the queue
    /// is empty, not busy.
    pub(crate) async fn next(&self, clock: &dyn Clock) -> (Slot, Work) {
        loop {
            if let Some(next) = self.pop() {
                return next;
            }
            self.kick.wait(clock, IDLE_WAIT).await;
        }
    }
}
