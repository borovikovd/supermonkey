//! App time and the core's activity.
//!
//! **App time.** The app runs on the system clock and Tokio's timers. The E2E
//! server installs a [`Manual`] clock instead: app time stands still until a
//! test moves it, and moving it fires the sleeps that came due. Every time
//! check and every sleep in the core goes through this module.
//!
//! **Activity.** Work that can still change state counts as busy: tasks
//! started with [`spawn`] or [`busy_scope`], queued writer jobs and effects
//! ([`Tracked`]), kicks not yet taken ([`Kick`]), and timers that fired. A task
//! waiting in [`sleep`], [`Kick::wait`], or [`parked`] is not busy until its
//! timer or kick fires. So when nothing is busy, the core stays still until
//! time moves or something arrives from outside (a UI action, a Gmail
//! response). The E2E server waits for that before a test asserts.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::model::Millis;

// ------------------------------------------------------------------ time

static MANUAL: OnceLock<Manual> = OnceLock::new();

/// App time in Unix milliseconds.
pub fn now_ms() -> Millis {
    MANUAL.get().map_or_else(system_ms, Manual::now)
}

fn system_ms() -> Millis {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// App time in the local time zone.
pub fn now_local() -> chrono::DateTime<chrono::Local> {
    chrono::DateTime::from_timestamp_millis(now_ms())
        .map_or_else(chrono::Local::now, |d| d.with_timezone(&chrono::Local))
}

/// Waits `d` of app time. The task is not busy while it waits.
pub async fn sleep(d: Duration) {
    // Held until the task has resumed, so its activity never drops to zero between.
    let _fired = parked(timer(d)).await;
}

/// The timer under [`sleep`] and [`Kick::wait`]. On the manual clock it
/// returns the activity of its firing, for the caller to drop once resumed.
async fn timer(d: Duration) -> Option<Busy> {
    if let Some(clock) = MANUAL.get() {
        Some(clock.sleep(d).await)
    } else {
        tokio::time::sleep(d).await;
        None
    }
}

/// A clock that moves only when told to. App time (the wall clock) and the
/// time sleeps count against move together, except in [`Manual::jump`], which
/// is the Mac asleep: the wall clock moves and no timer notices.
pub struct Manual {
    state: Mutex<ManualState>,
}

struct ManualState {
    /// App time, Unix ms.
    wall: Millis,
    /// Elapsed time for sleeps, ms.
    mono: u64,
    next_id: u64,
    /// By deadline, then by start order.
    sleepers: BTreeMap<(u64, u64), Arc<Sleeper>>,
}

#[derive(Default)]
struct Sleeper {
    /// Set when the timer fires; taken by the sleeping task.
    fired: Mutex<Option<Busy>>,
    notify: Notify,
}

impl Manual {
    /// Installs the manual clock at `wall_ms`, before anything reads the time.
    /// Fails if a clock is already installed.
    pub fn install(wall_ms: Millis) -> anyhow::Result<&'static Manual> {
        let clock = Manual {
            state: Mutex::new(ManualState {
                wall: wall_ms,
                mono: 0,
                next_id: 0,
                sleepers: BTreeMap::new(),
            }),
        };
        MANUAL
            .set(clock)
            .map_err(|_installed| anyhow::anyhow!("a manual clock is already installed"))?;
        MANUAL
            .get()
            .ok_or_else(|| anyhow::anyhow!("manual clock missing"))
    }

    fn now(&self) -> Millis {
        self.state.lock().wall
    }

    async fn sleep(&self, d: Duration) -> Busy {
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        if ms == 0 {
            return Busy::new("timer");
        }
        let sleeper = Arc::new(Sleeper::default());
        let key = {
            let mut s = self.state.lock();
            let id = s.next_id;
            s.next_id += 1;
            let key = (s.mono.saturating_add(ms), id);
            s.sleepers.insert(key, Arc::clone(&sleeper));
            key
        };
        let _registered = Registered { clock: self, key };
        loop {
            let notified = sleeper.notify.notified();
            let fired = sleeper.fired.lock().take();
            if let Some(fired) = fired {
                return fired;
            }
            notified.await;
        }
    }

    /// Moves time forward by `ms` and fires every sleep due by then, each once,
    /// as Playwright's `clock.fastForward` does. Returns how many fired.
    pub fn advance(&self, ms: u64) -> usize {
        let mut s = self.state.lock();
        let target = s.mono.saturating_add(ms);
        s.wall = s.wall.saturating_add(i64::try_from(ms).unwrap_or(i64::MAX));
        s.mono = target;
        let later = s.sleepers.split_off(&(target.saturating_add(1), 0));
        let due = std::mem::replace(&mut s.sleepers, later);
        drop(s);
        for sleeper in due.values() {
            *sleeper.fired.lock() = Some(Busy::new("timer"));
            sleeper.notify.notify_one();
        }
        due.len()
    }

    /// How long until the next sleep is due, if one is due within `ms`.
    pub fn next_due_within(&self, ms: u64) -> Option<u64> {
        let s = self.state.lock();
        let (&(deadline, _), _) = s.sleepers.first_key_value()?;
        let wait = deadline.saturating_sub(s.mono);
        (wait <= ms).then_some(wait)
    }

    /// Moves the wall clock without firing anything: the Mac was asleep for
    /// `ms`. Sleeps resume where they were, as monotonic timers do.
    pub fn jump(&self, ms: u64) {
        let mut s = self.state.lock();
        s.wall = s.wall.saturating_add(i64::try_from(ms).unwrap_or(i64::MAX));
    }
}

/// Removes a sleep from the clock when it ends or is dropped.
struct Registered<'a> {
    clock: &'a Manual,
    key: (u64, u64),
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.clock.state.lock().sleepers.remove(&self.key);
    }
}

// ------------------------------------------------------------- activity

static ACTIVITY: Activity = Activity {
    busy: Mutex::new(BTreeMap::new()),
    changed: Notify::const_new(),
};

struct Activity {
    /// Work in progress, by kind.
    busy: Mutex<BTreeMap<&'static str, usize>>,
    changed: Notify,
}

impl Activity {
    fn add(&self, label: &'static str) {
        *self.busy.lock().entry(label).or_default() += 1;
        self.changed.notify_waiters();
    }

    fn sub(&self, label: &'static str) {
        let mut busy = self.busy.lock();
        if let Some(n) = busy.get_mut(label) {
            *n -= 1;
            if *n == 0 {
                busy.remove(label);
            }
        }
        drop(busy);
        self.changed.notify_waiters();
    }
}

/// Work in progress now, by kind. Empty when the core is idle.
pub fn activity() -> BTreeMap<&'static str, usize> {
    ACTIVITY.busy.lock().clone()
}

/// Resolves on the next change in activity. Enable it before reading [`activity`].
pub fn activity_changed() -> tokio::sync::futures::Notified<'static> {
    ACTIVITY.changed.notified()
}

/// One unit of work in progress, counted until dropped.
#[must_use]
pub struct Busy(&'static str);

impl Busy {
    pub fn new(label: &'static str) -> Busy {
        ACTIVITY.add(label);
        Busy(label)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        ACTIVITY.sub(self.0);
    }
}

/// A queued value, busy until its receiver is done with it.
pub struct Tracked<T> {
    value: T,
    busy: Busy,
}

impl<T> Tracked<T> {
    pub fn new(label: &'static str, value: T) -> Tracked<T> {
        Tracked {
            value,
            busy: Busy::new(label),
        }
    }

    /// The value, and its activity to drop once it is handled.
    pub fn into_parts(self) -> (T, Busy) {
        (self.value, self.busy)
    }
}

/// A task's own activity: busy unless it is parked.
struct Task {
    label: &'static str,
    parked: AtomicBool,
}

impl Task {
    fn new(label: &'static str) -> Arc<Task> {
        ACTIVITY.add(label);
        Arc::new(Task {
            label,
            parked: AtomicBool::new(false),
        })
    }

    fn park(&self) {
        if !self.parked.swap(true, Ordering::AcqRel) {
            ACTIVITY.sub(self.label);
        }
    }

    fn unpark(&self) {
        if self.parked.swap(false, Ordering::AcqRel) {
            ACTIVITY.add(self.label);
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if !*self.parked.get_mut() {
            ACTIVITY.sub(self.label);
        }
    }
}

tokio::task_local! {
    static TASK: Arc<Task>;
}

/// Spawns `fut` on the app's runtime, busy until it finishes or is aborted.
pub fn spawn<F>(label: &'static str, fut: F) -> tauri::async_runtime::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tauri::async_runtime::spawn(TASK.scope(Task::new(label), fut))
}

/// Runs `fut` in the current task, busy until it finishes or is dropped.
pub async fn busy_scope<F: Future>(label: &'static str, fut: F) -> F::Output {
    TASK.scope(Task::new(label), fut).await
}

/// Awaits `fut` without counting the current task as busy. Only for waits
/// whose wake-up carries its own activity: a [`Kick`], a timer, a
/// [`Tracked`] item.
pub async fn parked<F: Future>(fut: F) -> F::Output {
    let _unpark = TASK.try_with(Arc::clone).ok().map(|task| {
        task.park();
        Unpark(task)
    });
    fut.await
}

struct Unpark(Arc<Task>);

impl Drop for Unpark {
    fn drop(&mut self) {
        self.0.unpark();
    }
}

/// From now until it ends, the current task waits on the person, not on the
/// core: a sign-in they may never finish in the browser.
pub fn waiting_on_person() {
    if TASK.try_with(|task| task.park()).is_err() {
        log::debug!("waiting_on_person outside a tracked task");
    }
}

/// Wakes a task that waits in [`Kick::wait`]. Like `Notify::notify_one`, a
/// kick with no waiter is kept for the next wait. Busy until a waiter takes it:
/// as `kick` when a task was waiting, or as `kick-deferred` when its task was
/// busy elsewhere and takes it at its next wait.
#[derive(Default)]
pub struct Kick {
    notify: Notify,
    pending: Mutex<Option<Busy>>,
    waiting: AtomicUsize,
    /// Set by [`Kick::close`]: every wait from then on returns at once.
    closed: AtomicBool,
}

/// The label of a kick whose task was not waiting for it.
pub const DEFERRED_KICK: &str = "kick-deferred";

impl Kick {
    pub fn kick(&self) {
        let label = if self.waiting.load(Ordering::SeqCst) > 0 {
            "kick"
        } else {
            DEFERRED_KICK
        };
        self.pending.lock().get_or_insert_with(|| Busy::new(label));
        self.notify.notify_one();
    }

    /// Wakes every waiter now and keeps nothing for later.
    pub fn kick_all(&self) {
        self.notify.notify_waiters();
    }

    /// Wakes every waiter, and makes every later wait return at once: the
    /// loop that waits here is shutting down. A waiter between its last check
    /// and its wait still wakes, which `kick_all` alone does not promise.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Waits for a kick, or for `timeout` of app time.
    pub async fn wait(&self, timeout: Duration) {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let _waiting = Waiting(&self.waiting);
        let _fired = parked(async {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Registered before the check, so a close in between still wakes it.
            notified.as_mut().enable();
            if self.closed.load(Ordering::SeqCst) {
                return None;
            }
            tokio::select! {
                () = notified => None,
                fired = timer(timeout) => fired,
            }
        })
        .await;
        drop(self.pending.lock().take());
    }
}

/// Counts a task in [`Kick::wait`] until the wait ends or is dropped.
struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn busy(label: &str) -> usize {
        activity().get(label).copied().unwrap_or(0)
    }

    /// Yields until `done`, failing instead of hanging when it never happens.
    async fn until(what: &str, done: impl Fn() -> bool) {
        for _ in 0..1000 {
            if done() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("never happened: {what}");
    }

    #[tokio::test]
    async fn a_task_is_busy_until_it_parks_and_again_once_kicked() {
        let kick = Arc::new(Kick::default());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let k = Arc::clone(&kick);
        let task = tokio::spawn(TASK.scope(Task::new("test-kick"), async move {
            k.wait(Duration::from_secs(3600)).await;
            let _ = tx.send(busy("test-kick"));
        }));
        until("the task parks", || busy("test-kick") == 0).await;
        kick.kick();
        // Busy from the kick until the task has resumed and finished.
        assert!(busy("test-kick") + busy("kick") >= 1);
        assert_eq!(rx.recv().await, Some(1));
        task.await.unwrap();
        assert_eq!(busy("test-kick"), 0);
        assert_eq!(busy("kick"), 0, "a taken kick stays busy");
    }

    #[tokio::test]
    async fn a_closed_kick_wakes_its_waiter_and_every_later_wait() {
        let kick = Arc::new(Kick::default());
        let k = Arc::clone(&kick);
        let waiter = tokio::spawn(async move { k.wait(Duration::from_secs(3600)).await });
        until("the waiter waits", || {
            kick.waiting.load(Ordering::SeqCst) == 1
        })
        .await;
        kick.close();
        waiter.await.unwrap();
        // A loop that checked before the close and waits after it: no hour-long wait.
        tokio::time::timeout(Duration::from_secs(5), kick.wait(Duration::from_secs(3600)))
            .await
            .expect("a wait after close returns at once");
    }

    fn manual(wall: Millis) -> Manual {
        Manual {
            state: Mutex::new(ManualState {
                wall,
                mono: 0,
                next_id: 0,
                sleepers: BTreeMap::new(),
            }),
        }
    }

    #[tokio::test]
    async fn advance_fires_due_sleeps_once_and_jump_fires_none() {
        let clock = Arc::new(manual(1_000));
        let started = |ms: u64| {
            let c = Arc::clone(&clock);
            tokio::spawn(async move { drop(c.sleep(Duration::from_millis(ms)).await) })
        };
        let (short, long) = (started(500), started(2_000));
        until("both sleeps register", || {
            clock.state.lock().sleepers.len() == 2
        })
        .await;
        assert_eq!(clock.next_due_within(400), None);
        assert_eq!(clock.next_due_within(500), Some(500));
        clock.jump(10_000);
        assert_eq!(clock.now(), 11_000);
        assert_eq!(clock.advance(499), 0);
        assert_eq!(clock.advance(1), 1);
        short.await.unwrap();
        assert!(!long.is_finished());
        assert_eq!(clock.advance(5_000), 1);
        long.await.unwrap();
        assert_eq!(clock.now(), 16_500);
    }

    #[tokio::test]
    async fn a_dropped_sleep_leaves_no_deadline_behind() {
        let clock = Arc::new(manual(0));
        let c = Arc::clone(&clock);
        let sleeping = tokio::spawn(async move { drop(c.sleep(Duration::from_millis(300)).await) });
        until("the sleep registers", || {
            clock.next_due_within(300) == Some(300)
        })
        .await;
        sleeping.abort();
        until("the sleep is dropped", || sleeping.is_finished()).await;
        assert_eq!(clock.next_due_within(u64::MAX), None);
        assert_eq!(clock.advance(300), 0);
    }

    #[test]
    fn tracked_items_are_busy_until_handled() {
        let item = Tracked::new("test-item", 7);
        assert_eq!(busy("test-item"), 1);
        let (value, done) = item.into_parts();
        assert_eq!(value, 7);
        assert_eq!(busy("test-item"), 1);
        drop(done);
        assert_eq!(busy("test-item"), 0);
    }
}
