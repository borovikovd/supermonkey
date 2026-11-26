#[cfg(any(test, feature = "fake"))]
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Local, Offset as _, TimeZone as _, Utc};
use parking_lot::Mutex;
#[cfg(any(test, feature = "fake"))]
use tokio::sync::oneshot;
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::domain::Millis;

#[derive(Debug, Clone)]
pub enum Clock {
    System,
    #[cfg(any(test, feature = "fake"))]
    Manual(Arc<ManualClock>),
}

impl Clock {
    #[must_use]
    pub const fn system() -> Self {
        Self::System
    }

    #[cfg(any(test, feature = "fake"))]
    #[must_use]
    pub fn manual(start: Millis) -> Self {
        Self::Manual(Arc::new(ManualClock::new(start)))
    }

    #[must_use]
    pub fn now(&self) -> Millis {
        match self {
            Self::System => Millis(Utc::now().timestamp_millis()),
            #[cfg(any(test, feature = "fake"))]
            Self::Manual(m) => m.now(),
        }
    }

    #[must_use]
    pub fn local_now(&self) -> DateTime<Local> {
        Local
            .timestamp_millis_opt(self.now().0)
            .single()
            .unwrap_or_else(Local::now)
    }

    #[must_use]
    pub fn local_offset(&self) -> FixedOffset {
        let now = self.now().0;
        Local
            .timestamp_millis_opt(now)
            .single()
            .map_or_else(|| Local::now().offset().fix(), |dt| dt.offset().fix())
    }

    #[must_use]
    pub fn sleep(&self, dur: Duration) -> ClockSleep {
        match self {
            Self::System => ClockSleep::System(Box::pin(tokio::time::sleep(dur))),
            #[cfg(any(test, feature = "fake"))]
            Self::Manual(m) => m.sleep(dur),
        }
    }

    #[must_use]
    pub fn sleep_until(&self, at: Millis) -> ClockSleep {
        let now = self.now().0;
        if at.0 <= now {
            return ClockSleep::Ready;
        }
        let ms = u64::try_from(at.0.saturating_sub(now)).unwrap_or(0);
        self.sleep(Duration::from_millis(ms))
    }

    #[cfg(any(test, feature = "fake"))]
    #[must_use]
    pub fn advance(&self, ms: u64) -> Millis {
        match self {
            Self::System => self.now(),
            Self::Manual(m) => m.advance(ms),
        }
    }

    #[cfg(any(test, feature = "fake"))]
    #[must_use]
    pub fn jump(&self, ms: u64) -> Millis {
        match self {
            Self::System => self.now(),
            Self::Manual(m) => m.jump(ms),
        }
    }

    #[cfg(any(test, feature = "fake"))]
    pub async fn advance_stepwise<F, Fut>(&self, ms: u64, mut settle: F) -> Millis
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = ()>,
    {
        match self {
            Self::System => self.now(),
            Self::Manual(m) => {
                let target = m.monotonic().saturating_add(ms);
                loop {
                    let cur = m.monotonic();
                    if cur >= target {
                        break;
                    }
                    let next = m.next_deadline_within(target).unwrap_or(target);
                    let step = next.saturating_sub(cur);
                    if step > 0 {
                        m.advance(step);
                    }
                    settle().await;
                }
                m.now()
            }
        }
    }
}

pub enum ClockSleep {
    Ready,
    System(Pin<Box<tokio::time::Sleep>>),
    #[cfg(any(test, feature = "fake"))]
    Manual(oneshot::Receiver<()>),
}

impl std::fmt::Debug for ClockSleep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ready => f.write_str("ClockSleep::Ready"),
            Self::System(_) => f.write_str("ClockSleep::System"),
            #[cfg(any(test, feature = "fake"))]
            Self::Manual(_) => f.write_str("ClockSleep::Manual"),
        }
    }
}

impl Future for ClockSleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        match &mut *self {
            Self::Ready => Poll::Ready(()),
            Self::System(s) => s.as_mut().poll(cx),
            #[cfg(any(test, feature = "fake"))]
            Self::Manual(rx) => match Pin::new(rx).poll(cx) {
                Poll::Ready(_) => Poll::Ready(()),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

#[cfg(any(test, feature = "fake"))]
#[derive(Debug)]
pub struct ManualClock {
    inner: Mutex<ManualState>,
}

#[cfg(any(test, feature = "fake"))]
#[derive(Debug)]
struct ManualState {
    wall: i64,
    monotonic: u64,
    timers: BTreeMap<u64, Vec<oneshot::Sender<()>>>,
}

#[cfg(any(test, feature = "fake"))]
impl ManualClock {
    const fn new(start: Millis) -> Self {
        Self {
            inner: Mutex::new(ManualState {
                wall: start.0,
                monotonic: 0,
                timers: BTreeMap::new(),
            }),
        }
    }

    fn now(&self) -> Millis {
        Millis(self.inner.lock().wall)
    }

    fn monotonic(&self) -> u64 {
        self.inner.lock().monotonic
    }

    fn sleep(&self, dur: Duration) -> ClockSleep {
        let ms = u64::try_from(dur.as_millis()).unwrap_or(u64::MAX);
        if ms == 0 {
            return ClockSleep::Ready;
        }
        let (tx, rx) = oneshot::channel();
        let mut state = self.inner.lock();
        let deadline = state.monotonic.saturating_add(ms);
        state.timers.entry(deadline).or_default().push(tx);
        ClockSleep::Manual(rx)
    }

    fn advance(&self, ms: u64) -> Millis {
        let mut to_wake = Vec::new();
        let now = {
            let mut state = self.inner.lock();
            let delta = i64::try_from(ms).unwrap_or(i64::MAX);
            state.wall = state.wall.saturating_add(delta);
            state.monotonic = state.monotonic.saturating_add(ms);
            let cut = state.monotonic;
            let rest = state.timers.split_off(&cut.saturating_add(1));
            let due = std::mem::replace(&mut state.timers, rest);
            for (_, list) in due {
                to_wake.extend(list);
            }
            Millis(state.wall)
        };
        for tx in to_wake {
            if tx.send(()).is_err() {}
        }
        now
    }

    fn jump(&self, ms: u64) -> Millis {
        let mut state = self.inner.lock();
        let delta = i64::try_from(ms).unwrap_or(i64::MAX);
        state.wall = state.wall.saturating_add(delta);
        Millis(state.wall)
    }

    fn next_deadline_within(&self, target: u64) -> Option<u64> {
        let mut state = self.inner.lock();
        state.timers.retain(|_, list| {
            list.retain(|tx| !tx.is_closed());
            !list.is_empty()
        });
        state.timers.keys().next().copied().filter(|&d| d <= target)
    }
}

#[derive(Debug, Default, Clone)]
pub struct Activity {
    inner: Arc<ActivityInner>,
}

#[derive(Debug, Default)]
struct ActivityInner {
    busy: AtomicUsize,
    sign_ins: AtomicUsize,
    idle: Notify,
}

#[derive(Debug)]
pub struct Busy {
    inner: Arc<ActivityInner>,
}

impl Drop for Busy {
    fn drop(&mut self) {
        if self.inner.busy.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.idle.notify_waiters();
        }
    }
}

#[derive(Debug)]
pub(crate) struct SignInGuard {
    inner: Arc<ActivityInner>,
}

impl Drop for SignInGuard {
    fn drop(&mut self) {
        self.inner.sign_ins.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Activity {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn enter(&self) -> Busy {
        self.inner.busy.fetch_add(1, Ordering::AcqRel);
        Busy {
            inner: Arc::clone(&self.inner),
        }
    }

    #[must_use]
    pub(crate) fn enter_sign_in(&self) -> SignInGuard {
        self.inner.sign_ins.fetch_add(1, Ordering::AcqRel);
        SignInGuard {
            inner: Arc::clone(&self.inner),
        }
    }

    #[cfg(any(test, feature = "fake"))]
    #[must_use]
    pub fn busy_count(&self) -> usize {
        self.inner.busy.load(Ordering::Acquire)
    }

    #[cfg(any(test, feature = "fake"))]
    #[must_use]
    pub fn sign_in_count(&self) -> usize {
        self.inner.sign_ins.load(Ordering::Acquire)
    }

    #[cfg(any(test, feature = "fake"))]
    pub async fn wait_idle(&self, timeout: Duration) -> bool {
        let wait = async {
            loop {
                let notified = self.inner.idle.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.inner.busy.load(Ordering::Acquire) == 0 {
                    tokio::task::yield_now().await;
                    if self.inner.busy.load(Ordering::Acquire) == 0 {
                        return;
                    }
                    continue;
                }
                notified.await;
            }
        };
        tokio::time::timeout(timeout, wait).await.is_ok()
    }
}

#[derive(Clone)]
pub(crate) struct Tasks {
    inner: Arc<TasksInner>,
}

struct TasksInner {
    handle: tokio::runtime::Handle,
    token: CancellationToken,
    tracker: TaskTracker,
    activity: Activity,
    children: Mutex<Vec<Tasks>>,
}

impl std::fmt::Debug for Tasks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tasks")
            .field("cancelled", &self.inner.token.is_cancelled())
            .field("tasks", &self.inner.tracker.len())
            .finish_non_exhaustive()
    }
}

impl Tasks {
    #[must_use]
    pub(crate) fn new(activity: Activity) -> Self {
        Self {
            inner: Arc::new(TasksInner {
                handle: tokio::runtime::Handle::current(),
                token: CancellationToken::new(),
                tracker: TaskTracker::new(),
                activity,
                children: Mutex::new(Vec::new()),
            }),
        }
    }

    #[must_use]
    pub(crate) fn child(&self) -> Self {
        let child = Self {
            inner: Arc::new(TasksInner {
                handle: self.inner.handle.clone(),
                token: self.inner.token.child_token(),
                tracker: TaskTracker::new(),
                activity: self.inner.activity.clone(),
                children: Mutex::new(Vec::new()),
            }),
        };
        let mut children = self.inner.children.lock();
        children.retain(|c| !c.inner.tracker.is_closed() || !c.inner.tracker.is_empty());
        children.push(child.clone());
        child
    }

    #[must_use]
    pub(crate) fn token(&self) -> CancellationToken {
        self.inner.token.clone()
    }

    pub(crate) fn spawn<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if self.inner.token.is_cancelled() || self.inner.tracker.is_closed() {
            return;
        }
        self.inner.tracker.spawn_on(fut, &self.inner.handle);
    }

    pub(crate) fn spawn_busy<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if self.inner.token.is_cancelled() || self.inner.tracker.is_closed() {
            return;
        }
        let guard = self.inner.activity.enter();
        self.inner.tracker.spawn_on(
            async move {
                fut.await;
                drop(guard);
            },
            &self.inner.handle,
        );
    }

    pub(crate) fn stop(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.inner.token.cancel();
            self.inner.tracker.close();
            let children = std::mem::take(&mut *self.inner.children.lock());
            for child in children {
                child.stop().await;
            }
            self.inner.tracker.wait().await;
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitOutcome {
    Kicked,
    Elapsed,
    Cancelled,
}

#[derive(Debug, Default)]
struct KickState {
    pending: bool,
    running: bool,
    busy: Option<Busy>,
}

#[derive(Debug)]
pub(crate) struct KickGuard {
    state: Arc<Mutex<KickState>>,
    activity: Activity,
    _busy: Busy,
}

impl Drop for KickGuard {
    fn drop(&mut self) {
        let mut st = self.state.lock();
        st.running = false;
        if st.pending && st.busy.is_none() {
            st.busy = Some(self.activity.enter());
        }
    }
}

#[derive(Debug)]
pub(crate) struct Kick {
    state: Arc<Mutex<KickState>>,
    notify: Notify,
}

impl Default for Kick {
    fn default() -> Self {
        Self::new()
    }
}

impl Kick {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(KickState::default())),
            notify: Notify::new(),
        }
    }

    pub(crate) fn wake(&self, activity: &Activity) {
        {
            let mut st = self.state.lock();
            st.pending = true;
            if !st.running && st.busy.is_none() {
                st.busy = Some(activity.enter());
            }
        }
        self.notify.notify_one();
    }

    fn clear_busy(&self) {
        let mut st = self.state.lock();
        st.pending = false;
        st.busy.take();
    }

    fn take_pending(&self, activity: &Activity) -> Option<KickGuard> {
        let mut st = self.state.lock();
        if !st.pending {
            return None;
        }
        st.pending = false;
        st.running = true;
        let busy = st.busy.take().unwrap_or_else(|| activity.enter());
        Some(KickGuard {
            state: Arc::clone(&self.state),
            activity: activity.clone(),
            _busy: busy,
        })
    }

    fn enter_elapsed(&self, activity: &Activity) -> KickGuard {
        let mut st = self.state.lock();
        st.pending = false;
        st.running = true;
        let busy = st.busy.take().unwrap_or_else(|| activity.enter());
        KickGuard {
            state: Arc::clone(&self.state),
            activity: activity.clone(),
            _busy: busy,
        }
    }

    pub(crate) async fn wait(
        &self,
        activity: &Activity,
        cancel: &CancellationToken,
    ) -> Option<KickGuard> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(guard) = self.take_pending(activity) {
                return Some(guard);
            }
            if cancel.is_cancelled() {
                self.clear_busy();
                return None;
            }
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    self.clear_busy();
                    return None;
                }
                () = notified => {}
            }
        }
    }

    pub(crate) async fn wait_timeout(
        &self,
        clock: &Clock,
        dur: Duration,
        activity: &Activity,
        cancel: &CancellationToken,
    ) -> (WaitOutcome, Option<KickGuard>) {
        let sleep = clock.sleep(dur);
        tokio::pin!(sleep);
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(guard) = self.take_pending(activity) {
                return (WaitOutcome::Kicked, Some(guard));
            }
            if cancel.is_cancelled() {
                self.clear_busy();
                return (WaitOutcome::Cancelled, None);
            }
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    self.clear_busy();
                    return (WaitOutcome::Cancelled, None);
                }
                () = notified => {}
                () = &mut sleep => {
                    return (WaitOutcome::Elapsed, Some(self.enter_elapsed(activity)));
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Signal<T> {
    tx: Arc<watch::Sender<T>>,
}

impl<T: Clone + PartialEq + Send + Sync + 'static> Signal<T> {
    #[must_use]
    pub(crate) fn new(initial: T) -> Self {
        let (tx, _rx) = watch::channel(initial);
        Self { tx: Arc::new(tx) }
    }

    #[must_use]
    pub(crate) fn get(&self) -> T {
        self.tx.borrow().clone()
    }

    pub(crate) fn set(&self, value: T) -> bool {
        self.tx.send_if_modified(|cur| {
            if *cur == value {
                false
            } else {
                *cur = value;
                true
            }
        })
    }

    #[must_use]
    pub(crate) fn subscribe(&self) -> watch::Receiver<T> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;

    #[tokio::test]
    async fn activity_tracks_busy_and_tasks_waits_for_children() {
        let activity = Activity::new();
        assert_eq!(activity.busy_count(), 0);
        let root = Tasks::new(activity.clone());
        let child = root.child();
        let grandchild = child.child();
        let finished = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&finished);
        let cancel = grandchild.token();

        grandchild.spawn_busy(async move {
            cancel.cancelled().await;
            tokio::task::yield_now().await;
            done.store(true, Ordering::SeqCst);
        });
        assert_eq!(activity.busy_count(), 1);

        root.stop().await;
        assert!(finished.load(Ordering::SeqCst));
        assert!(activity.wait_idle(Duration::from_secs(1)).await);
    }

    #[tokio::test]
    async fn manual_clock_advance_wakes_timers_and_jump_does_not() {
        let clock = Clock::manual(Millis(1_000));
        let mut sleep = Box::pin(clock.sleep(Duration::from_millis(500)));
        assert_eq!(clock.jump(10_000), Millis(11_000));
        assert_eq!(clock.now(), Millis(11_000));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut sleep)
                .await
                .is_err()
        );
        assert_eq!(clock.advance(499), Millis(11_499));
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut sleep)
                .await
                .is_err()
        );
        assert_eq!(clock.advance(1), Millis(11_500));
        sleep.await;
        assert_eq!(clock.now(), Millis(11_500));
    }
}
