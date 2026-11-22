use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use sm_domain::{GmailThreadId, LabelId};
use sm_gmail::{HistoryId, PageToken, Priority};

use crate::activity::{Activity, Kick};
use crate::clock::Clock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Slot {
    pub(crate) priority: Priority,
    rank: u8,
}

pub(crate) const POLL_META: Slot = Slot {
    priority: Priority::P3,
    rank: 0,
};
pub(crate) const POLL_BODIES: Slot = Slot {
    priority: Priority::P4,
    rank: 0,
};
pub(crate) const INBOX_META: Slot = Slot {
    priority: Priority::P4,
    rank: 1,
};
pub(crate) const OTHER_META: Slot = Slot {
    priority: Priority::P4,
    rank: 2,
};
pub(crate) const INBOX_BODIES: Slot = Slot {
    priority: Priority::P4,
    rank: 3,
};
pub(crate) const LATE: Slot = Slot {
    priority: Priority::P5,
    rank: 0,
};
pub(crate) const BACKFILL: Slot = Slot {
    priority: Priority::P6,
    rank: 0,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Listing {
    Label(LabelId),
    Query(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Work {
    List {
        listing: Listing,
        page: Option<PageToken>,
        meta: Slot,
        body: Option<Slot>,
    },
    Meta {
        thread: GmailThreadId,
        history: Option<HistoryId>,
        body: Option<Slot>,
    },
    Body {
        thread: GmailThreadId,
    },
    Backfill {
        page: Option<PageToken>,
    },
}

const IDLE_WAIT: Duration = Duration::from_secs(3600);

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

    pub(crate) async fn next(&self, clock: &dyn Clock) -> (Slot, Work) {
        loop {
            if let Some(next) = self.pop() {
                return next;
            }
            self.kick.wait(clock, IDLE_WAIT).await;
        }
    }
}
