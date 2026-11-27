use std::collections::{BTreeMap, HashMap};

use parking_lot::Mutex;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

use crate::domain::GmailThreadId;
use crate::gmail::{Format, Priority as GmailPriority};

use super::super::runtime::{Activity, Busy};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum FetchPriority {
    OpenConversation,
    SentThread,
    SyncNew,
    ConnectInbox,
    ConnectSecondary,
}

impl FetchPriority {
    #[must_use]
    pub(crate) const fn gmail(self) -> GmailPriority {
        match self {
            Self::OpenConversation | Self::SentThread => GmailPriority::P1,
            Self::SyncNew => GmailPriority::P2,
            Self::ConnectInbox => GmailPriority::P4,
            Self::ConnectSecondary => GmailPriority::P5,
        }
    }
}

#[derive(Debug)]
pub(crate) struct FetchJob {
    pub(crate) gmail_id: GmailThreadId,
    pub(crate) format: Format,
    pub(crate) priority: FetchPriority,
    pub(crate) notify_new_mail: bool,
    barriers: Vec<mpsc::UnboundedSender<()>>,
    _busy: Vec<Busy>,
}

struct QueuedEntry {
    seq: u64,
    priority: FetchPriority,
    format: Format,
    notify_new_mail: bool,
    barriers: Vec<mpsc::UnboundedSender<()>>,
    busy: Vec<Busy>,
}

struct InFlightEntry {
    format: Format,
    requeue: Option<QueuedEntry>,
    barriers: Vec<mpsc::UnboundedSender<()>>,
}

struct QueueInner {
    next_seq: u64,
    by_id: HashMap<GmailThreadId, QueuedEntry>,
    order: BTreeMap<(FetchPriority, u64), GmailThreadId>,
    in_flight: HashMap<GmailThreadId, InFlightEntry>,
}

pub(crate) struct FetchQueue {
    inner: Mutex<QueueInner>,
    notify: Notify,
}

impl std::fmt::Debug for FetchQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let g = self.inner.lock();
        f.debug_struct("FetchQueue")
            .field("queued", &g.by_id.len())
            .field("in_flight", &g.in_flight.len())
            .finish_non_exhaustive()
    }
}

impl Default for FetchQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl FetchQueue {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(QueueInner {
                next_seq: 0,
                by_id: HashMap::new(),
                order: BTreeMap::new(),
                in_flight: HashMap::new(),
            }),
            notify: Notify::new(),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "deduplicating priority enqueue handles in-flight promotion, queued promotion, and fresh inserts"
    )]
    pub(crate) fn enqueue(
        &self,
        gmail_id: GmailThreadId,
        format: Format,
        priority: FetchPriority,
        notify_new_mail: bool,
        barrier: Option<mpsc::UnboundedSender<()>>,
        activity: &Activity,
    ) {
        let mut g = self.inner.lock();
        if let Some(flight_fmt) = g.in_flight.get(&gmail_id).map(|f| f.format) {
            let needs_requeue = matches!(
                (flight_fmt, format),
                (Format::Metadata | Format::Minimal, Format::Full)
            ) || notify_new_mail
                || priority <= FetchPriority::SyncNew;
            if needs_requeue {
                let busy = activity.enter();
                let next_seq = g.next_seq.saturating_add(1);
                let mut used_seq = false;
                if let Some(flight) = g.in_flight.get_mut(&gmail_id) {
                    if let Some(rq) = &mut flight.requeue {
                        rq.priority = rq.priority.min(priority);
                        rq.format = merge_format(rq.format, format);
                        rq.notify_new_mail |= notify_new_mail;
                        if let Some(b) = barrier {
                            rq.barriers.push(b);
                        }
                        rq.busy.push(busy);
                    } else {
                        used_seq = true;
                        flight.requeue = Some(QueuedEntry {
                            seq: next_seq,
                            priority,
                            format,
                            notify_new_mail,
                            barriers: barrier.into_iter().collect(),
                            busy: vec![busy],
                        });
                    }
                }
                if used_seq {
                    g.next_seq = next_seq;
                }
            } else if let Some(b) = barrier
                && let Some(flight) = g.in_flight.get_mut(&gmail_id)
            {
                flight.barriers.push(b);
            }
            return;
        }

        if let Some(existing) = g.by_id.get_mut(&gmail_id) {
            let old_key = (existing.priority, existing.seq);
            let new_prio = existing.priority.min(priority);
            existing.format = merge_format(existing.format, format);
            existing.notify_new_mail |= notify_new_mail;
            if let Some(b) = barrier {
                existing.barriers.push(b);
            }
            if new_prio != existing.priority {
                existing.priority = new_prio;
                let new_key = (new_prio, existing.seq);
                g.order.remove(&old_key);
                g.order.insert(new_key, gmail_id);
            }
            return;
        }

        g.next_seq = g.next_seq.saturating_add(1);
        let seq = g.next_seq;
        g.by_id.insert(
            gmail_id.clone(),
            QueuedEntry {
                seq,
                priority,
                format,
                notify_new_mail,
                barriers: barrier.into_iter().collect(),
                busy: vec![activity.enter()],
            },
        );
        g.order.insert((priority, seq), gmail_id);
        drop(g);
        self.notify.notify_one();
    }

    pub(crate) async fn pop(&self, cancel: &CancellationToken) -> Option<FetchJob> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(job) = self.try_pop() {
                return Some(job);
            }
            if cancel.is_cancelled() {
                return None;
            }
            tokio::select! {
                biased;
                () = cancel.cancelled() => return None,
                () = notified => {}
            }
        }
    }

    fn try_pop(&self) -> Option<FetchJob> {
        let mut g = self.inner.lock();
        let (_, gmail_id) = g.order.pop_first()?;
        let entry = g.by_id.remove(&gmail_id)?;
        g.in_flight.insert(
            gmail_id.clone(),
            InFlightEntry {
                format: entry.format,
                requeue: None,
                barriers: Vec::new(),
            },
        );
        Some(FetchJob {
            gmail_id,
            format: entry.format,
            priority: entry.priority,
            notify_new_mail: entry.notify_new_mail,
            barriers: entry.barriers,
            _busy: entry.busy,
        })
    }

    pub(crate) fn finish(&self, job: &FetchJob, settled: bool) {
        let mut g = self.inner.lock();
        let flight = g.in_flight.remove(&job.gmail_id);
        let (requeue, late_barriers) =
            flight.map_or((None, Vec::new()), |f| (f.requeue, f.barriers));
        let requeued = requeue.is_some();
        if let Some(entry) = requeue {
            let key = (entry.priority, entry.seq);
            g.by_id.insert(job.gmail_id.clone(), entry);
            g.order.insert(key, job.gmail_id.clone());
        }
        drop(g);
        if requeued {
            self.notify.notify_one();
        }
        if settled {
            for barrier in job.barriers.iter().chain(&late_barriers) {
                if barrier.send(()).is_err() {}
            }
        }
    }

    pub(crate) fn clear(&self) {
        let mut g = self.inner.lock();
        g.by_id.clear();
        g.order.clear();
        for flight in g.in_flight.values_mut() {
            flight.requeue = None;
        }
    }
}

const fn merge_format(a: Format, b: Format) -> Format {
    match (a, b) {
        (Format::Full, _) | (_, Format::Full) => Format::Full,
        (Format::Metadata, _) | (_, Format::Metadata) => Format::Metadata,
        (Format::Minimal, Format::Minimal) => Format::Minimal,
    }
}
