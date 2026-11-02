//! Background fetches in priority order (SPEC Sync). Each item runs through
//! the account's quota scheduler at its priority. Within a priority, a rank
//! orders the stages: poll bodies, inbox metadata, drafts and starred
//! metadata, inbox bodies.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use tokio::sync::Notify;
use ub_gmail::{Format, Priority, ThreadListQuery, cost};

use super::{AccountRt, EngineRef};

pub const WORKERS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slot {
    pub priority: u8,
    pub rank: u8,
}

pub const POLL_BODIES: Slot = Slot {
    priority: 4,
    rank: 0,
};
pub const INBOX_META: Slot = Slot {
    priority: 4,
    rank: 1,
};
pub const OTHER_META: Slot = Slot {
    priority: 4,
    rank: 2,
};
pub const INBOX_BODIES: Slot = Slot {
    priority: 4,
    rank: 3,
};
pub const LATE_BODIES: Slot = Slot {
    priority: 5,
    rank: 0,
};
pub const BACKFILL: Slot = Slot {
    priority: 6,
    rank: 0,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Work {
    /// A `threads.list` page; each listed thread gets `meta`, then `body` if set.
    List {
        label: Option<String>,
        q: Option<String>,
        page: Option<String>,
        meta: Slot,
        body: Option<Slot>,
        include_spam_trash: bool,
    },
    /// Thread metadata, skipped when the cached thread's historyId matches.
    Meta {
        thread: String,
        history: Option<u64>,
        body: Option<Slot>,
    },
    /// Every body the thread is missing. Stored bodies are never fetched again.
    Body { thread: String },
    /// Contacts backfill from the last year of sent mail.
    Backfill { page: Option<String> },
}

#[derive(Default)]
pub struct WorkQueue {
    inner: Mutex<Inner>,
    kick: Notify,
    closed: AtomicBool,
}

#[derive(Default)]
struct Inner {
    heap: BinaryHeap<Reverse<(Slot, u64)>>,
    items: HashMap<u64, Work>,
    queued: HashSet<Work>,
    seq: u64,
}

impl WorkQueue {
    pub fn push(&self, slot: Slot, work: Work) {
        let mut q = self.inner.lock();
        if !q.queued.insert(work.clone()) {
            return;
        }
        q.seq += 1;
        let seq = q.seq;
        q.items.insert(seq, work);
        q.heap.push(Reverse((slot, seq)));
        drop(q);
        self.kick.notify_one();
    }

    fn pop(&self) -> Option<(Slot, Work)> {
        let mut q = self.inner.lock();
        let Reverse((slot, seq)) = q.heap.pop()?;
        let work = q.items.remove(&seq)?;
        q.queued.remove(&work);
        Some((slot, work))
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.kick.notify_waiters();
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().heap.is_empty()
    }
}

pub async fn worker(e: EngineRef, rt: Arc<AccountRt>) {
    loop {
        if rt.work.closed.load(Ordering::Relaxed) || rt.is_stopped() {
            return;
        }
        let Some((slot, work)) = rt.work.pop() else {
            super::wait(&rt.work.kick, 5_000).await;
            continue;
        };
        let mut attempt = 0;
        loop {
            match run(&e, &rt, slot, &work).await {
                Ok(()) => {
                    e.note(&rt, Ok(()));
                    break;
                }
                Err(err) => {
                    e.note(&rt, Err(&err));
                    if rt.is_stopped() {
                        return;
                    }
                    match err.outcome {
                        // Gone or not ours to fetch: drop it, no toast.
                        ub_gmail::Outcome::NotFound | ub_gmail::Outcome::Rejected { .. } => break,
                        ub_gmail::Outcome::InvalidGrant => return,
                        ub_gmail::Outcome::Offline => {
                            // Wait for the network, then retry.
                            super::wait(&rt.poll_kick, 10_000).await;
                        }
                        _ => {
                            let d = ub_gmail::backoff(attempt, err.retry_after());
                            attempt += 1;
                            tokio::time::sleep(d).await;
                        }
                    }
                }
            }
        }
    }
}

fn prio(slot: Slot) -> Priority {
    Priority::new(slot.priority)
}

async fn run(
    e: &EngineRef,
    rt: &Arc<AccountRt>,
    slot: Slot,
    work: &Work,
) -> Result<(), ub_gmail::GmailError> {
    match work {
        Work::List {
            label,
            q,
            page,
            meta,
            body,
            include_spam_trash,
        } => {
            let query = ThreadListQuery {
                q: q.clone(),
                label_ids: label.iter().cloned().collect(),
                page_token: page.clone(),
                max_results: Some(500),
                include_spam_trash: *include_spam_trash,
            };
            let res = rt
                .sched
                .run(
                    prio(slot),
                    cost::THREADS_LIST,
                    rt.gmail.list_threads(&query),
                )
                .await?;
            for t in res.threads {
                rt.work.push(
                    *meta,
                    Work::Meta {
                        thread: t.id,
                        history: t.history_id,
                        body: *body,
                    },
                );
            }
            if let Some(next) = res.next_page_token {
                rt.work.push(
                    slot,
                    Work::List {
                        label: label.clone(),
                        q: q.clone(),
                        page: Some(next),
                        meta: *meta,
                        body: *body,
                        include_spam_trash: *include_spam_trash,
                    },
                );
            }
            Ok(())
        }
        Work::Meta {
            thread,
            history,
            body,
        } => {
            let cached = super::sync::cached_history(e, rt.id, thread);
            let fresh = matches!((cached, history), (Some(c), Some(h)) if c == *h);
            if !fresh {
                super::sync::fetch_thread(e, rt, thread, Format::Metadata, prio(slot)).await?;
            }
            if let Some(b) = body {
                rt.work.push(
                    *b,
                    Work::Body {
                        thread: thread.clone(),
                    },
                );
            }
            Ok(())
        }
        Work::Body { thread } => {
            if super::sync::needs_bodies(e, rt.id, thread) {
                super::sync::fetch_thread(e, rt, thread, Format::Full, prio(slot)).await?;
            }
            Ok(())
        }
        Work::Backfill { page } => super::contacts::backfill_page(e, rt, page.clone()).await,
    }
}
