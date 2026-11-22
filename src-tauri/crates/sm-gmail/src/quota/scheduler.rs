use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio::time::Instant;

use super::bucket::{BackgroundWindow, Bucket, Spend};
use super::{Priority, QuotaConfig, Units};

#[derive(Debug)]
pub struct Scheduler {
    config: QuotaConfig,
    accounts: Mutex<HashMap<String, AccountScheduler>>,
}

impl Scheduler {
    #[must_use]
    pub fn new(config: QuotaConfig) -> Self {
        Self {
            config,
            accounts: Mutex::default(),
        }
    }

    #[must_use]
    pub fn account(&self, email: &str) -> AccountScheduler {
        let now = Instant::now();
        self.accounts
            .lock()
            .entry(email.to_owned())
            .or_insert_with(|| AccountScheduler::new(self.config, now))
            .clone()
    }

    pub fn forget(&self, email: &str) {
        let forgotten = self.accounts.lock().remove(email);
        drop(forgotten);
    }
}

#[derive(Debug, Clone)]
pub struct AccountScheduler {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    config: QuotaConfig,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    budget: Budget,
    waiters: BTreeMap<WaiterKey, Waiter>,
}

#[derive(Debug)]
struct Budget {
    bucket: Bucket,
    background: BackgroundWindow,
    in_flight: usize,
    next_ticket: u64,
}

type WaiterKey = (Priority, u64);

#[derive(Debug)]
struct Waiter {
    cost: Units,
    notify: Arc<Notify>,
    stage: Stage,
}

#[derive(Debug)]
enum Stage {
    Waiting { wake_at: Option<Instant> },
    Granted { spend: Option<u64> },
}

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
                    budget: Budget {
                        bucket: Bucket::new(config.capacity, config.refill_per_minute, now),
                        background: BackgroundWindow::default(),
                        in_flight: 0,
                        next_ticket: 0,
                    },
                    waiters: BTreeMap::new(),
                }),
            }),
        }
    }

    pub async fn acquire(&self, priority: Priority, cost: Units) -> Permit {
        let notify = Arc::new(Notify::new());
        let now = Instant::now();
        let key = {
            let mut state = self.shared.state.lock();
            let key = (priority, state.budget.ticket());
            state.waiters.insert(
                key,
                Waiter {
                    cost,
                    notify: Arc::clone(&notify),
                    stage: Stage::Waiting { wake_at: None },
                },
            );
            let woken = state.pump(self.shared.config, now);
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

    #[must_use]
    pub fn bucket_level(&self) -> f64 {
        let now = Instant::now();
        self.shared.state.lock().budget.bucket.level(now)
    }

    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.shared.state.lock().budget.in_flight
    }

    #[must_use]
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
    fn pump(&self, now: Instant) {
        let woken = self.state.lock().pump(self.config, now);
        wake(woken);
    }
}

impl Budget {
    const fn ticket(&mut self) -> u64 {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        ticket
    }

    fn admit(
        &mut self,
        config: QuotaConfig,
        priority: Priority,
        cost: Units,
        now: Instant,
    ) -> Admit {
        if self.in_flight >= config.max_in_flight {
            return Admit::WhenSlotFrees;
        }
        let mut needed = f64::from(cost.0.min(config.capacity.0));
        if priority.is_background() {
            let spent = self.background.spent(now);
            if spent + u64::from(cost.0) > u64::from(config.background_per_minute.0)
                && let Some(expiry) = self.background.next_expiry()
            {
                return Admit::At(expiry);
            }
            needed = needed.max(f64::from(config.background_reserve.0) + 1.0);
        }
        if self.bucket.has(needed, now) {
            Admit::Now
        } else {
            Admit::At(now + self.bucket.time_until(needed, now))
        }
    }

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

impl State {
    fn pump(&mut self, config: QuotaConfig, now: Instant) -> Vec<Arc<Notify>> {
        let mut woken = Vec::new();
        let in_line = self
            .waiters
            .iter_mut()
            .filter(|(_, w)| matches!(w.stage, Stage::Waiting { .. }));
        for (&(priority, _), waiter) in in_line {
            match self.budget.admit(config, priority, waiter.cost, now) {
                Admit::Now => {
                    let spend = self.budget.start(priority, waiter.cost, now);
                    waiter.stage = Stage::Granted { spend };
                    woken.push(Arc::clone(&waiter.notify));
                }
                Admit::WhenSlotFrees => {
                    waiter.stage = Stage::Waiting { wake_at: None };
                    break;
                }
                Admit::At(at) => {
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
}

fn wake(woken: Vec<Arc<Notify>>) {
    for notify in woken {
        notify.notify_one();
    }
}

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
            state.budget.in_flight -= 1;
            state.budget.bucket.give_back(cost, now);
            if let Some(ticket) = spend {
                state.budget.background.cancel(ticket);
            }
        }
        let woken = state.pump(self.shared.config, now);
        drop(state);
        wake(woken);
    }
}

#[must_use = "the slot frees as soon as the permit drops"]
#[derive(Debug)]
pub struct Permit {
    shared: Arc<Shared>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let now = Instant::now();
        let mut state = self.shared.state.lock();
        state.budget.in_flight -= 1;
        let woken = state.pump(self.shared.config, now);
        drop(state);
        wake(woken);
    }
}
