//! Per-account quota budget and request scheduling (SPEC: Sync).
//!
//! Each account has a [`QuotaBucket`] (1,000 units, refilling 4,800 a minute, so no minute
//! spends more than about 5,800) and at most eight requests in flight. Requests wait in
//! priority order, 1 first. Priorities 4–6 spend at most 4,000 units in any 60 s window and
//! only while the bucket holds more than 500 units; priorities 1–3 may drain it.
//!
//! Time is `tokio::time::Instant`, so tests can use a paused clock.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, MutexGuard};
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::types::Format;

/// Gmail quota units per call, from Google's published table.
pub mod cost {
    use super::Format;

    pub const GET_PROFILE: u32 = 1;
    pub const SEND_AS_LIST: u32 = 1;
    pub const LABELS_LIST: u32 = 1;
    pub const LABELS_CREATE: u32 = 5;
    pub const HISTORY_LIST: u32 = 2;
    pub const THREADS_LIST: u32 = 10;
    pub const MESSAGES_BATCH_MODIFY: u32 = 50;
    pub const MESSAGES_TRASH: u32 = 20;
    pub const ATTACHMENTS_GET: u32 = 20;
    pub const DRAFTS_LIST: u32 = 5;
    pub const DRAFTS_CREATE: u32 = 10;
    pub const DRAFTS_UPDATE: u32 = 15;
    pub const DRAFTS_SEND: u32 = 100;
    pub const DRAFTS_DELETE: u32 = 10;

    /// `threads.get`: 40, or 120 with `format=full`.
    pub fn threads_get(format: Format) -> u32 {
        match format {
            Format::Full | Format::Raw => 120,
            Format::Minimal | Format::Metadata => 40,
        }
    }

    /// `messages.get`: 20, or 60 with `format=full` or `raw`.
    pub fn messages_get(format: Format) -> u32 {
        match format {
            Format::Full | Format::Raw => 60,
            Format::Minimal | Format::Metadata => 20,
        }
    }

    /// `drafts.get`: priced like `messages.get`.
    pub fn drafts_get(format: Format) -> u32 {
        messages_get(format)
    }
}

/// A request priority, 1 (highest) to 6. See SPEC, Sync, for what runs at each level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Priority(u8);

impl Priority {
    /// UI actions, sends, draft upserts.
    pub const P1: Priority = Priority(1);
    /// Opened threads, search, label views, attachments, draft reconciliation.
    pub const P2: Priority = Priority(2);
    /// History polls and their metadata fetches.
    pub const P3: Priority = Priority(3);
    /// Poll bodies, inbox metadata and bodies, drafts and starred metadata.
    pub const P4: Priority = Priority(4);
    /// Search-hit bodies, Sent, draft and starred bodies.
    pub const P5: Priority = Priority(5);
    /// The contacts backfill.
    pub const P6: Priority = Priority(6);

    /// A priority from its level. Panics outside 1..=6.
    pub const fn new(level: u8) -> Priority {
        assert!(matches!(level, 1..=6), "priority must be 1..=6");
        Priority(level)
    }

    /// The level, 1..=6.
    pub fn level(self) -> u8 {
        self.0
    }

    /// Priorities 4–6, which keep the bucket's reserve for 1–3.
    pub fn is_background(self) -> bool {
        self.0 >= 4
    }
}

/// The budget and concurrency limits, per account. Build-time config in the app.
#[derive(Debug, Clone)]
pub struct QuotaConfig {
    /// Bucket size in units.
    pub capacity: u32,
    /// Units added per minute.
    pub refill_per_minute: u32,
    /// Most units priorities 4–6 may spend in any 60 s.
    pub background_per_minute: u32,
    /// Priorities 4–6 run only while the bucket holds more than this.
    pub background_reserve: u32,
    /// Most requests in flight per account.
    pub max_in_flight: usize,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        QuotaConfig {
            capacity: 1000,
            refill_per_minute: 4800,
            background_per_minute: 4000,
            background_reserve: 500,
            max_in_flight: 8,
        }
    }
}

/// A token bucket of quota units that starts full.
#[derive(Debug, Clone)]
pub struct QuotaBucket {
    capacity: f64,
    refill_per_sec: f64,
    level: f64,
    updated: Instant,
}

/// Rounding slack when comparing a refilled level with a target.
const EPSILON: f64 = 1e-6;

impl QuotaBucket {
    /// A full bucket.
    pub fn new(capacity: u32, refill_per_minute: u32) -> Self {
        QuotaBucket {
            capacity: f64::from(capacity),
            refill_per_sec: f64::from(refill_per_minute) / 60.0,
            level: f64::from(capacity),
            updated: Instant::now(),
        }
    }

    /// Units available now.
    pub fn level(&mut self) -> f64 {
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.updated).as_secs_f64();
        self.level = (self.level + elapsed * self.refill_per_sec).min(self.capacity);
        self.updated = now;
        self.level
    }

    /// Whether the bucket holds at least `units`.
    pub fn has(&mut self, units: f64) -> bool {
        self.level() + EPSILON >= units
    }

    /// Spends `units`. The level may go negative; the debt refills like any deficit.
    pub fn take(&mut self, units: u32) {
        self.level();
        self.level -= f64::from(units);
    }

    /// Returns `units` that were taken but not used.
    pub fn give_back(&mut self, units: u32) {
        self.level();
        self.level = (self.level + f64::from(units)).min(self.capacity);
    }

    /// When the bucket will hold `units`.
    pub fn time_until(&mut self, units: f64) -> Duration {
        let missing = (units - self.level()).max(0.0);
        Duration::from_secs_f64(missing / self.refill_per_sec)
    }
}

/// Requests for every account. Clone it freely; it is a handle.
#[derive(Clone)]
pub struct Scheduler {
    inner: Arc<Inner>,
}

struct Inner {
    cfg: QuotaConfig,
    state: Mutex<State>,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock()
    }
}

#[derive(Default)]
struct State {
    accounts: HashMap<Arc<str>, AccountState>,
    waiters: HashMap<u64, Waiter>,
    next_id: u64,
    on_screen: Option<String>,
    /// When the first waiter blocked on budget can run.
    next_wake: Option<Instant>,
}

struct Waiter {
    account: Arc<str>,
    priority: Priority,
    cost: u32,
    granted: bool,
    /// The `background_spent` entry its grant added, if background.
    spent: Option<(Instant, u32)>,
    notify: Arc<Notify>,
}

struct AccountState {
    bucket: QuotaBucket,
    /// Spending of priorities 4–6 in the last 60 s.
    background_spent: VecDeque<(Instant, u32)>,
    in_flight: usize,
}

enum Admit {
    Now,
    WhenSlotFrees,
    At(Instant),
}

const WINDOW: Duration = Duration::from_secs(60);

impl AccountState {
    fn new(cfg: &QuotaConfig) -> Self {
        AccountState {
            bucket: QuotaBucket::new(cfg.capacity, cfg.refill_per_minute),
            background_spent: VecDeque::new(),
            in_flight: 0,
        }
    }

    fn check(&mut self, cfg: &QuotaConfig, priority: Priority, cost: u32) -> Admit {
        if self.in_flight >= cfg.max_in_flight {
            return Admit::WhenSlotFrees;
        }
        let now = Instant::now();
        // A call costing more than the bucket holds runs once the bucket is full.
        let mut needed = f64::from(cost.min(cfg.capacity));
        if priority.is_background() {
            while self
                .background_spent
                .front()
                .is_some_and(|&(at, _)| now.duration_since(at) >= WINDOW)
            {
                self.background_spent.pop_front();
            }
            let spent: u32 = self.background_spent.iter().map(|(_, units)| units).sum();
            if spent + cost > cfg.background_per_minute
                && let Some(&(oldest, _)) = self.background_spent.front()
            {
                return Admit::At(oldest + WINDOW);
            }
            // "More than the reserve", in whole units.
            needed = needed.max(f64::from(cfg.background_reserve) + 1.0);
        }
        if self.bucket.has(needed) {
            Admit::Now
        } else {
            Admit::At(now + self.bucket.time_until(needed))
        }
    }

    /// Takes a slot and the units. Returns the background spending recorded.
    fn start(&mut self, priority: Priority, cost: u32) -> Option<(Instant, u32)> {
        self.bucket.take(cost);
        self.in_flight += 1;
        let spent = priority.is_background().then(|| (Instant::now(), cost));
        self.background_spent.extend(spent);
        spent
    }

    /// Undoes [`AccountState::start`] for a request that never ran.
    fn cancel(&mut self, cost: u32, spent: Option<(Instant, u32)>) {
        self.in_flight -= 1;
        self.bucket.give_back(cost);
        if let Some(entry) = spent
            && let Some(i) = self.background_spent.iter().rposition(|e| *e == entry)
        {
            self.background_spent.remove(i);
        }
    }
}

impl State {
    /// Starts every waiter that may run now, in order: priority, then the account on
    /// screen, then arrival. Once an account's best waiter is blocked, that account's
    /// later waiters wait too, so a cheap request never overtakes an expensive one.
    fn pump(&mut self, cfg: &QuotaConfig) {
        let on_screen = self.on_screen.as_deref();
        let mut order: Vec<(Priority, bool, u64)> = self
            .waiters
            .iter()
            .filter(|(_, w)| !w.granted)
            .map(|(&id, w)| (w.priority, Some(&*w.account) != on_screen, id))
            .collect();
        order.sort_unstable();

        let mut blocked = HashSet::new();
        let mut next_wake: Option<Instant> = None;
        for (_, _, id) in order {
            // Listed just above, under the same lock.
            let Some(waiter) = self.waiters.get_mut(&id) else {
                continue;
            };
            if blocked.contains(&waiter.account) {
                continue;
            }
            let account = account_state(&mut self.accounts, &waiter.account, cfg);
            match account.check(cfg, waiter.priority, waiter.cost) {
                Admit::Now => {
                    waiter.spent = account.start(waiter.priority, waiter.cost);
                    waiter.granted = true;
                    waiter.notify.notify_one();
                }
                Admit::WhenSlotFrees => {
                    blocked.insert(Arc::clone(&waiter.account));
                }
                Admit::At(at) => {
                    blocked.insert(Arc::clone(&waiter.account));
                    next_wake = Some(next_wake.map_or(at, |t| t.min(at)));
                    // It may be sleeping without a timer (it was waiting for a slot).
                    // Wake it so it arms one.
                    waiter.notify.notify_one();
                }
            }
        }
        self.next_wake = next_wake;
    }
}

/// The account's state, created on first use.
fn account_state<'a>(
    accounts: &'a mut HashMap<Arc<str>, AccountState>,
    account: &Arc<str>,
    cfg: &QuotaConfig,
) -> &'a mut AccountState {
    accounts
        .entry(Arc::clone(account))
        .or_insert_with(|| AccountState::new(cfg))
}

impl Scheduler {
    /// A scheduler with the given per-account limits.
    pub fn new(cfg: QuotaConfig) -> Self {
        Scheduler {
            inner: Arc::new(Inner {
                cfg,
                state: Mutex::new(State::default()),
            }),
        }
    }

    /// The handle for one account's requests.
    pub fn account(&self, email: impl Into<Arc<str>>) -> AccountScheduler {
        AccountScheduler {
            inner: Arc::clone(&self.inner),
            account: email.into(),
        }
    }

    /// Marks the account on screen. Its requests go first among equal priorities.
    pub fn set_on_screen(&self, email: Option<&str>) {
        let mut state = self.inner.lock();
        state.on_screen = email.map(str::to_owned);
        state.pump(&self.inner.cfg);
    }
}

/// Schedules one account's requests. Cheap to clone.
#[derive(Clone)]
pub struct AccountScheduler {
    inner: Arc<Inner>,
    account: Arc<str>,
}

impl AccountScheduler {
    /// Waits for a slot and `cost` units at `priority`, then runs `request`.
    ///
    /// Dropping the returned future cancels it: a queued request leaves the queue, and a
    /// running one frees its slot. Search uses this to cancel older queued fetches.
    pub async fn run<F: Future>(&self, priority: Priority, cost: u32, request: F) -> F::Output {
        let _permit = self.acquire(priority, cost).await;
        request.await
    }

    /// Waits for a slot and budget. The slot is held until the [`Permit`] drops.
    pub async fn acquire(&self, priority: Priority, cost: u32) -> Permit {
        let notify = Arc::new(Notify::new());
        let id = {
            let mut state = self.inner.lock();
            let id = state.next_id;
            state.next_id += 1;
            state.waiters.insert(
                id,
                Waiter {
                    account: Arc::clone(&self.account),
                    priority,
                    cost,
                    granted: false,
                    spent: None,
                    notify: Arc::clone(&notify),
                },
            );
            state.pump(&self.inner.cfg);
            id
        };
        let mut queued = Queued {
            inner: &self.inner,
            id,
            done: false,
        };
        loop {
            let next_wake = {
                let mut state = self.inner.lock();
                if state.waiters[&id].granted {
                    state.waiters.remove(&id);
                    queued.done = true;
                    return Permit {
                        inner: Arc::clone(&self.inner),
                        account: Arc::clone(&self.account),
                    };
                }
                state.next_wake
            };
            match next_wake {
                Some(at) => {
                    tokio::select! {
                        () = notify.notified() => {}
                        () = tokio::time::sleep_until(at) => {
                            self.inner.lock().pump(&self.inner.cfg);
                        }
                    }
                }
                None => notify.notified().await,
            }
        }
    }

    /// Units in this account's bucket now.
    pub fn bucket_level(&self) -> f64 {
        let mut state = self.inner.lock();
        account_state(&mut state.accounts, &self.account, &self.inner.cfg)
            .bucket
            .level()
    }

    /// Requests of this account running now.
    pub fn in_flight(&self) -> usize {
        let state = self.inner.lock();
        state.accounts.get(&self.account).map_or(0, |a| a.in_flight)
    }

    /// Requests of this account waiting for a slot or budget.
    pub fn queued(&self) -> usize {
        let state = self.inner.lock();
        state
            .waiters
            .values()
            .filter(|w| !w.granted && w.account == self.account)
            .count()
    }
}

/// Removes a waiter whose `acquire` future was dropped. If it had just been granted, its
/// slot, units, and background spending go back.
struct Queued<'a> {
    inner: &'a Inner,
    id: u64,
    done: bool,
}

impl Drop for Queued<'_> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let mut state = self.inner.lock();
        if let Some(waiter) = state.waiters.remove(&self.id)
            && waiter.granted
            && let Some(account) = state.accounts.get_mut(&waiter.account)
        {
            account.cancel(waiter.cost, waiter.spent);
        }
        state.pump(&self.inner.cfg);
    }
}

/// A running request's slot. Dropping it lets the next request start.
pub struct Permit {
    inner: Arc<Inner>,
    account: Arc<str>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.inner.lock();
        if let Some(account) = state.accounts.get_mut(&self.account) {
            account.in_flight -= 1;
        }
        state.pump(&self.inner.cfg);
    }
}
