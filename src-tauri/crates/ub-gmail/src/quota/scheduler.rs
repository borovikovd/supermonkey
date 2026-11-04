//! Admitting requests per account: slots, budget, and priority order.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio::time::Instant;

use super::bucket::{BackgroundWindow, Bucket, Spend};
use super::{Priority, QuotaConfig, Units};

/// The schedulers of every account. A cheap handle: clones share the accounts.
#[derive(Clone)]
pub struct Scheduler {
    config: QuotaConfig,
    accounts: Arc<Mutex<HashMap<String, AccountScheduler>>>,
}

impl Scheduler {
    /// A scheduler that gives every account `config`'s limits.
    pub fn new(config: QuotaConfig) -> Self {
        Self {
            config,
            accounts: Arc::default(),
        }
    }

    /// The scheduler of the account `email`, made on first use. Gmail's quota
    /// is per user, so every client of one address shares one scheduler.
    pub fn account(&self, email: &str) -> AccountScheduler {
        // Read before locking: no clock calls under a lock.
        let now = Instant::now();
        self.accounts
            .lock()
            .entry(email.to_owned())
            .or_insert_with(|| AccountScheduler::new(self.config, now))
            .clone()
    }

    /// Forgets the account `email`: it was removed, and its clients are
    /// gone. Handles still held keep working on their own; the next
    /// [`Scheduler::account`] for the address starts afresh.
    pub fn forget(&self, email: &str) {
        let forgotten = self.accounts.lock().remove(email);
        drop(forgotten);
    }
}

/// One account's slots and budget. A cheap handle: clones share the account.
#[derive(Clone)]
pub struct AccountScheduler {
    shared: Arc<Shared>,
}

struct Shared {
    config: QuotaConfig,
    state: Mutex<State>,
}

/// Everything that must agree, under one lock.
struct State {
    bucket: Bucket,
    background: BackgroundWindow,
    in_flight: usize,
    /// Waiters by priority, then arrival.
    waiters: BTreeMap<WaiterKey, Waiter>,
    next_ticket: u64,
}

/// A waiter's place in line: priority first, then arrival.
type WaiterKey = (Priority, u64);

struct Waiter {
    cost: Units,
    notify: Arc<Notify>,
    stage: Stage,
}

enum Stage {
    /// In line. `wake_at` is set when this waiter is first in line and must
    /// look again at that time, as budget refills or background spends expire.
    Waiting { wake_at: Option<Instant> },
    /// Holds a slot and its units; `spend` is its background spending.
    Granted { spend: Option<u64> },
}

/// Whether the first waiter in line may start.
enum Admit {
    Now,
    WhenSlotFrees,
    At(Instant),
}

impl AccountScheduler {
    fn new(config: QuotaConfig, now: Instant) -> Self {
        Self {
            shared: Arc::new(Shared {
                config,
                state: Mutex::new(State {
                    bucket: Bucket::new(config.capacity, config.refill_per_minute, now),
                    background: BackgroundWindow::default(),
                    in_flight: 0,
                    waiters: BTreeMap::new(),
                    next_ticket: 0,
                }),
            }),
        }
    }

    /// Waits for a slot and `cost` units at `priority`, then runs `request`.
    ///
    /// Dropping the returned future cancels it: a queued request leaves the
    /// line, and a running one frees its slot.
    pub async fn run<F: Future>(&self, priority: Priority, cost: Units, request: F) -> F::Output {
        let _permit = self.acquire(priority, cost).await;
        request.await
    }

    /// Waits for a slot and `cost` units at `priority`. The slot is held until
    /// the [`Permit`] drops; the units are spent.
    ///
    /// Cancellation-safe: dropping the future before it resolves gives back
    /// everything it was granted.
    pub async fn acquire(&self, priority: Priority, cost: Units) -> Permit {
        let notify = Arc::new(Notify::new());
        let now = Instant::now();
        let key = {
            let mut state = self.shared.state.lock();
            let key = (priority, state.ticket());
            state.waiters.insert(
                key,
                Waiter {
                    cost,
                    notify: Arc::clone(&notify),
                    stage: Stage::Waiting { wake_at: None },
                },
            );
            let woken = state.pump(&self.shared.config, now);
            drop(state);
            wake(woken);
            key
        };
        let mut in_line = InLine {
            shared: &self.shared,
            key: Some(key),
        };
        loop {
            let wake_at = {
                let mut state = self.shared.state.lock();
                match state.waiters.get(&key).map(|w| &w.stage) {
                    Some(Stage::Granted { .. }) => {
                        state.waiters.remove(&key);
                        in_line.key = None;
                        return Permit {
                            shared: Arc::clone(&self.shared),
                        };
                    }
                    Some(Stage::Waiting { wake_at }) => *wake_at,
                    // Only this future removes its own entry.
                    None => None,
                }
            };
            match wake_at {
                Some(at) => {
                    tokio::select! {
                        () = notify.notified() => {}
                        () = tokio::time::sleep_until(at) => self.shared.pump(Instant::now()),
                    }
                }
                None => notify.notified().await,
            }
        }
    }

    /// Units in the bucket now.
    pub fn bucket_level(&self) -> f64 {
        let now = Instant::now();
        self.shared.state.lock().bucket.level(now)
    }

    /// Requests running now.
    pub fn in_flight(&self) -> usize {
        self.shared.state.lock().in_flight
    }

    /// Requests waiting for a slot or budget.
    pub fn queued(&self) -> usize {
        self.shared
            .state
            .lock()
            .waiters
            .values()
            .filter(|w| matches!(w.stage, Stage::Waiting { .. }))
            .count()
    }
}

impl Shared {
    /// Starts what may start at `now`, then wakes those waiters outside the lock.
    fn pump(&self, now: Instant) {
        let woken = self.state.lock().pump(&self.config, now);
        wake(woken);
    }
}

impl State {
    fn ticket(&mut self) -> u64 {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        ticket
    }

    /// Whether a request may start at `now`.
    fn admit(
        &mut self,
        config: &QuotaConfig,
        priority: Priority,
        cost: Units,
        now: Instant,
    ) -> Admit {
        if self.in_flight >= config.max_in_flight {
            return Admit::WhenSlotFrees;
        }
        // A request that costs more than the bucket holds runs once it is full.
        let mut needed = f64::from(cost.0.min(config.capacity.0));
        if priority.is_background() {
            let spent = self.background.spent(now);
            if spent + u64::from(cost.0) > u64::from(config.background_per_minute.0)
                && let Some(expiry) = self.background.next_expiry()
            {
                return Admit::At(expiry);
            }
            // "More than the reserve", in whole units.
            needed = needed.max(f64::from(config.background_reserve.0) + 1.0);
        }
        if self.bucket.has(needed, now) {
            Admit::Now
        } else {
            Admit::At(now + self.bucket.time_until(needed, now))
        }
    }

    /// Starts every waiter that may start, in line order. Once the first
    /// waiter still in line is blocked, everyone behind it waits too, so a
    /// cheap request never overtakes an earlier one waiting for budget.
    /// Returns the waiters to wake, for the caller to wake after unlocking.
    fn pump(&mut self, config: &QuotaConfig, now: Instant) -> Vec<Arc<Notify>> {
        let mut woken = Vec::new();
        let keys: Vec<WaiterKey> = self
            .waiters
            .iter()
            .filter(|(_, w)| matches!(w.stage, Stage::Waiting { .. }))
            .map(|(&key, _)| key)
            .collect();
        for key in keys {
            let Some(cost) = self.waiters.get(&key).map(|w| w.cost) else {
                continue;
            };
            let (priority, _) = key;
            let admit = self.admit(config, priority, cost, now);
            let spend = match admit {
                Admit::Now => Some(self.start(priority, cost, now)),
                Admit::WhenSlotFrees | Admit::At(_) => None,
            };
            let Some(waiter) = self.waiters.get_mut(&key) else {
                continue;
            };
            match admit {
                Admit::Now => {
                    waiter.stage = Stage::Granted {
                        spend: spend.flatten(),
                    };
                    woken.push(Arc::clone(&waiter.notify));
                }
                Admit::WhenSlotFrees => {
                    // A freed slot pumps again; no timer needed.
                    waiter.stage = Stage::Waiting { wake_at: None };
                    break;
                }
                Admit::At(at) => {
                    // The waiter may be asleep without a timer: wake it to arm one.
                    if !matches!(waiter.stage, Stage::Waiting { wake_at: Some(t) } if t == at) {
                        waiter.stage = Stage::Waiting { wake_at: Some(at) };
                        woken.push(Arc::clone(&waiter.notify));
                    }
                    break;
                }
            }
        }
        woken
    }

    /// Takes a slot and the units. Returns the background spend's ticket.
    fn start(&mut self, priority: Priority, cost: Units, now: Instant) -> Option<u64> {
        self.bucket.take(cost, now);
        self.in_flight += 1;
        priority.is_background().then(|| {
            let ticket = self.ticket();
            self.background.record(Spend {
                at: now,
                units: cost,
                ticket,
            });
            ticket
        })
    }
}

fn wake(woken: Vec<Arc<Notify>>) {
    for notify in woken {
        notify.notify_one();
    }
}

/// Takes a waiter out of line when its `acquire` future is dropped. If it was
/// granted but never ran, its slot, units, and background spend go back.
struct InLine<'a> {
    shared: &'a Shared,
    key: Option<WaiterKey>,
}

impl Drop for InLine<'_> {
    fn drop(&mut self) {
        let Some(key) = self.key else {
            return;
        };
        let now = Instant::now();
        let mut state = self.shared.state.lock();
        if let Some(Waiter {
            cost,
            stage: Stage::Granted { spend },
            ..
        }) = state.waiters.remove(&key)
        {
            state.in_flight -= 1;
            state.bucket.give_back(cost, now);
            if let Some(ticket) = spend {
                state.background.cancel(ticket);
            }
        }
        let woken = state.pump(&self.shared.config, now);
        drop(state);
        wake(woken);
    }
}

/// A running request's slot. Dropping it lets the next request start.
#[must_use = "the slot frees as soon as the permit drops"]
pub struct Permit {
    shared: Arc<Shared>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let now = Instant::now();
        let mut state = self.shared.state.lock();
        state.in_flight -= 1;
        let woken = state.pump(&self.shared.config, now);
        drop(state);
        wake(woken);
    }
}
