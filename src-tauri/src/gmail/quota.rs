use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio::time::Instant;

use super::wire::Format;

const WINDOW_SECS: Duration = Duration::from_secs(60);
const EPSILON: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Units(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    P1,
    P2,
    P3,
    P4,
    P5,
    P6,
}

impl Priority {
    #[must_use]
    pub const fn is_background(self) -> bool {
        matches!(self, Self::P4 | Self::P5 | Self::P6)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaConfig {
    pub capacity: Units,
    pub refill_per_minute: Units,
    pub background_per_minute: Units,
    pub background_reserve: Units,
    pub max_in_flight: usize,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            capacity: Units(1000),
            refill_per_minute: Units(4800),
            background_per_minute: Units(4000),
            background_reserve: Units(500),
            max_in_flight: 8,
        }
    }
}

impl QuotaConfig {
    #[must_use]
    pub const fn for_budget(per_minute: Units) -> Self {
        let refill = per_minute.0;
        let bg = refill.saturating_mul(5) / 6;
        let cap = if refill / 4 > 1000 { 1000 } else { refill / 4 };
        let cap = if cap < 200 { 200 } else { cap };
        let reserve = cap / 2;
        Self {
            capacity: Units(cap),
            refill_per_minute: Units(refill),
            background_per_minute: Units(bg),
            background_reserve: Units(reserve),
            max_in_flight: 8,
        }
    }
}

pub(super) const COST_PROFILE: Units = Units(1);
pub(super) const COST_SEND_AS: Units = Units(1);
pub(super) const COST_LABELS_LIST: Units = Units(1);
pub(super) const COST_FILTERS_LIST: Units = Units(1);
pub(super) const COST_HISTORY: Units = Units(2);
pub(super) const COST_LABEL_CREATE: Units = Units(5);
pub(super) const COST_FILTER_WRITE: Units = Units(5);
pub(super) const COST_DRAFTS_LIST: Units = Units(5);
pub(super) const COST_THREADS_LIST: Units = Units(10);
pub(super) const COST_DRAFT_CREATE: Units = Units(10);
pub(super) const COST_DRAFT_DELETE: Units = Units(10);
pub(super) const COST_DRAFT_UPDATE: Units = Units(15);
pub(super) const COST_TRASH: Units = Units(20);
pub(super) const COST_ATTACHMENT: Units = Units(20);
pub(super) const COST_BATCH_MODIFY: Units = Units(50);
pub(super) const COST_SEND: Units = Units(100);

pub(super) const fn cost_message_get(format: Format) -> Units {
    match format {
        Format::Full => Units(60),
        Format::Metadata | Format::Minimal => Units(20),
    }
}

pub(super) const fn cost_thread_get(format: Format) -> Units {
    match format {
        Format::Full => Units(120),
        Format::Metadata | Format::Minimal => Units(40),
    }
}

#[derive(Debug, Clone)]
pub struct Scheduler {
    config: QuotaConfig,
    accounts: Arc<Mutex<HashMap<String, Arc<AccountScheduler>>>>,
}

impl Scheduler {
    #[must_use]
    pub fn new(config: QuotaConfig) -> Self {
        Self {
            config,
            accounts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn account(&self, email: &str) -> Arc<AccountScheduler> {
        let key = email.trim().to_ascii_lowercase();
        let mut map = self.accounts.lock();
        Arc::clone(
            map.entry(key)
                .or_insert_with(|| Arc::new(AccountScheduler::new(self.config))),
        )
    }

    pub fn forget(&self, email: &str) {
        self.accounts
            .lock()
            .remove(&email.trim().to_ascii_lowercase());
    }
}

struct Bucket {
    level: f64,
    capacity: f64,
    rate_per_sec: f64,
    updated: Instant,
}

impl Bucket {
    fn new(config: QuotaConfig, now: Instant) -> Self {
        let capacity = f64::from(config.capacity.0);
        Self {
            level: capacity,
            capacity,
            rate_per_sec: f64::from(config.refill_per_minute.0) / 60.0,
            updated: now,
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        if elapsed > 0.0 {
            self.level = elapsed
                .mul_add(self.rate_per_sec, self.level)
                .min(self.capacity);
            self.updated = now;
        }
    }

    fn time_until(&self, needed: f64, now: Instant) -> Option<Instant> {
        let deficit = needed - self.level;
        if deficit <= EPSILON {
            return None;
        }
        let secs = if self.rate_per_sec > 0.0 {
            deficit / self.rate_per_sec
        } else {
            60.0
        };
        Some(now + Duration::from_secs_f64(secs.max(0.001)))
    }
}

struct Spend {
    ticket: u64,
    at: Instant,
    cost: u32,
}

struct BackgroundWindow {
    limit: u32,
    entries: VecDeque<Spend>,
    used: u32,
}

impl BackgroundWindow {
    const fn new(limit: u32) -> Self {
        Self {
            limit,
            entries: VecDeque::new(),
            used: 0,
        }
    }

    fn prune(&mut self, now: Instant) {
        while let Some(front) = self.entries.front() {
            if now.saturating_duration_since(front.at) >= WINDOW_SECS {
                self.used = self.used.saturating_sub(front.cost);
                self.entries.pop_front();
            } else {
                break;
            }
        }
    }

    fn wait_until(&mut self, cost: u32, now: Instant) -> Option<Instant> {
        self.prune(now);
        if self.used.saturating_add(cost) <= self.limit || self.entries.is_empty() {
            return None;
        }
        let mut remaining = self.used;
        for entry in &self.entries {
            remaining = remaining.saturating_sub(entry.cost);
            if remaining.saturating_add(cost) <= self.limit {
                return Some(entry.at + WINDOW_SECS);
            }
        }
        self.entries.back().map(|e| e.at + WINDOW_SECS)
    }

    fn record(&mut self, ticket: u64, cost: u32, now: Instant) {
        self.entries.push_back(Spend {
            ticket,
            at: now,
            cost,
        });
        self.used = self.used.saturating_add(cost);
    }

    fn cancel(&mut self, ticket: u64) {
        if let Some(pos) = self.entries.iter().position(|e| e.ticket == ticket)
            && let Some(removed) = self.entries.remove(pos)
        {
            self.used = self.used.saturating_sub(removed.cost);
        }
    }
}

struct Waiter {
    cost: Units,
    background: bool,
    granted: bool,
    wake_at: Option<Instant>,
    notify: Arc<Notify>,
}

struct Budget {
    config: QuotaConfig,
    bucket: Bucket,
    bg_window: BackgroundWindow,
    in_flight: usize,
}

impl Budget {
    fn can_admit(&mut self, cost: Units, background: bool, now: Instant) -> Admit {
        if self.in_flight >= self.config.max_in_flight {
            return Admit::WhenSlotFrees;
        }
        self.bucket.refill(now);
        let cost_f = f64::from(cost.0);
        let needed = if background {
            let max_reserve = (self.bucket.capacity - 1.0).max(0.0);
            let reserve = f64::from(self.config.background_reserve.0).min(max_reserve);
            cost_f.min(self.bucket.capacity).max(reserve)
        } else {
            cost_f.min(self.bucket.capacity)
        };
        if let Some(at) = self.bucket.time_until(needed, now) {
            return Admit::At(at);
        }
        if background && let Some(at) = self.bg_window.wait_until(cost.0, now) {
            return Admit::At(at);
        }
        Admit::Now
    }
}

struct State {
    budget: Budget,
    next_seq: u64,
    queue: BTreeMap<(Priority, u64), Waiter>,
}

enum Admit {
    Now,
    WhenSlotFrees,
    At(Instant),
}

impl State {
    fn pump(&mut self, now: Instant) {
        let mut blocked = false;
        for (&(_, seq), w) in &mut self.queue {
            if w.granted {
                continue;
            }
            if blocked {
                w.wake_at = None;
                continue;
            }
            let (cost, bg) = (w.cost, w.background);
            match self.budget.can_admit(cost, bg, now) {
                Admit::Now => {
                    self.budget.in_flight += 1;
                    self.budget.bucket.level -= f64::from(cost.0);
                    if bg {
                        self.budget.bg_window.record(seq, cost.0, now);
                    }
                    w.granted = true;
                    w.wake_at = None;
                    w.notify.notify_one();
                }
                Admit::WhenSlotFrees => {
                    w.wake_at = None;
                    blocked = true;
                }
                Admit::At(at) => {
                    let changed = w.wake_at.is_none_or(|prev| {
                        let diff = if prev > at { prev - at } else { at - prev };
                        diff > Duration::from_millis(1)
                    });
                    w.wake_at = Some(at);
                    if changed {
                        w.notify.notify_one();
                    }
                    blocked = true;
                }
            }
        }
    }
}

pub struct AccountScheduler {
    state: Mutex<State>,
}

impl fmt::Debug for AccountScheduler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountScheduler").finish_non_exhaustive()
    }
}

impl AccountScheduler {
    #[must_use]
    pub fn new(config: QuotaConfig) -> Self {
        let now = Instant::now();
        Self {
            state: Mutex::new(State {
                budget: Budget {
                    config,
                    bucket: Bucket::new(config, now),
                    bg_window: BackgroundWindow::new(config.background_per_minute.0),
                    in_flight: 0,
                },
                next_seq: 0,
                queue: BTreeMap::new(),
            }),
        }
    }

    pub async fn acquire(self: &Arc<Self>, priority: Priority, cost: Units) -> Permit {
        let notify = Arc::new(Notify::new());
        let key = {
            let mut st = self.state.lock();
            st.next_seq = st.next_seq.saturating_add(1);
            let key = (priority, st.next_seq);
            st.queue.insert(
                key,
                Waiter {
                    cost,
                    background: priority.is_background(),
                    granted: false,
                    wake_at: None,
                    notify: Arc::clone(&notify),
                },
            );
            st.pump(Instant::now());
            key
        };

        let mut in_line = InLine {
            scheduler: Some(Arc::clone(self)),
            key,
            cost,
            background: priority.is_background(),
        };

        loop {
            let wake_at = {
                let mut st = self.state.lock();
                st.pump(Instant::now());
                if st.queue.get(&key).is_some_and(|w| w.granted) {
                    st.queue.remove(&key);
                    let scheduler = in_line.scheduler.take().unwrap_or_else(|| Arc::clone(self));
                    return Permit { scheduler };
                }
                st.queue.get(&key).and_then(|w| w.wake_at)
            };

            let notified = notify.notified();
            match wake_at {
                Some(deadline) => {
                    tokio::select! {
                        () = notified => {}
                        () = tokio::time::sleep_until(deadline) => {}
                    }
                }
                None => notified.await,
            }
        }
    }

    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.state.lock().budget.in_flight
    }

    #[must_use]
    pub fn queued(&self) -> usize {
        self.state.lock().queue.len()
    }

    #[must_use]
    pub fn bucket_level(&self) -> f64 {
        let mut st = self.state.lock();
        st.budget.bucket.refill(Instant::now());
        st.budget.bucket.level
    }
}

struct InLine {
    scheduler: Option<Arc<AccountScheduler>>,
    key: (Priority, u64),
    cost: Units,
    background: bool,
}

impl Drop for InLine {
    fn drop(&mut self) {
        let Some(scheduler) = self.scheduler.take() else {
            return;
        };
        let mut st = scheduler.state.lock();
        if let Some(waiter) = st.queue.remove(&self.key)
            && waiter.granted
        {
            st.budget.in_flight = st.budget.in_flight.saturating_sub(1);
            st.budget.bucket.level =
                (st.budget.bucket.level + f64::from(self.cost.0)).min(st.budget.bucket.capacity);
            if self.background {
                st.budget.bg_window.cancel(self.key.1);
            }
        }
        st.pump(Instant::now());
    }
}

pub struct Permit {
    scheduler: Arc<AccountScheduler>,
}

impl fmt::Debug for Permit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Permit").finish_non_exhaustive()
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut st = self.scheduler.state.lock();
        st.budget.in_flight = st.budget.in_flight.saturating_sub(1);
        st.pump(Instant::now());
    }
}
