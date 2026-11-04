//! Spawned tasks: each belongs to a cancellation token, and each counts as
//! busy unless it is parked on a wait whose wake-up carries its own activity.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tokio_util::task::task_tracker::TaskTrackerToken;

use super::Activity;

/// A task's own activity: busy unless it is parked.
struct TaskActivity {
    activity: Arc<Activity>,
    label: &'static str,
    parked: AtomicBool,
}

impl TaskActivity {
    fn new(activity: Arc<Activity>, label: &'static str) -> Arc<Self> {
        activity.add(label);
        Arc::new(Self {
            activity,
            label,
            parked: AtomicBool::new(false),
        })
    }

    fn park(&self) {
        if !self.parked.swap(true, Ordering::AcqRel) {
            self.activity.sub(self.label);
        }
    }

    fn unpark(&self) {
        if self.parked.swap(false, Ordering::AcqRel) {
            self.activity.add(self.label);
        }
    }
}

impl Drop for TaskActivity {
    fn drop(&mut self) {
        if !*self.parked.get_mut() {
            self.activity.sub(self.label);
        }
    }
}

tokio::task_local! {
    static TASK: Arc<TaskActivity>;
}

/// A group of tasks under one cancellation token: the core's root group, or
/// an account worker's. Cancelling it ends every task in it at its next
/// `.await`; [`Tasks::stop`] also waits for them to end, and for the tasks
/// of every group inside it.
#[derive(Clone)]
pub struct Tasks {
    runtime: Handle,
    activity: Arc<Activity>,
    cancel: CancellationToken,
    tracker: TaskTracker,
    /// The trackers of the groups this one is inside, innermost first: each
    /// counts this group's tasks too, so stopping it waits for them.
    outer: Arc<[TaskTracker]>,
}

impl Tasks {
    /// A root group on `runtime`, counting its tasks in `activity`.
    pub fn new(runtime: Handle, activity: Arc<Activity>) -> Self {
        Self {
            runtime,
            activity,
            cancel: CancellationToken::new(),
            tracker: TaskTracker::new(),
            outer: Arc::new([]),
        }
    }

    /// A group inside this one: cancelled with it, or on its own. Stopping
    /// this one waits for the new group's tasks too.
    #[must_use]
    pub fn child(&self) -> Self {
        let outer: Vec<TaskTracker> = std::iter::once(self.tracker.clone())
            .chain(self.outer.iter().cloned())
            .collect();
        Self {
            runtime: self.runtime.clone(),
            activity: Arc::clone(&self.activity),
            cancel: self.cancel.child_token(),
            tracker: TaskTracker::new(),
            outer: outer.into(),
        }
    }

    /// The activity tracker the group counts its tasks in.
    pub fn activity(&self) -> &Arc<Activity> {
        &self.activity
    }

    /// The group's token: cancelled once the group stops.
    pub fn token(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Spawns `task`, busy as `label` until it ends or parks. It ends early,
    /// dropped at its next `.await`, when the group is cancelled. After that
    /// nothing new starts.
    pub fn spawn<F>(&self, label: &'static str, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if self.cancel.is_cancelled() {
            tracing::debug!(task = label, "not started: the group has stopped");
            return;
        }
        let cancel = self.cancel.clone();
        // Counted by every outer group until the task has ended.
        let counted: Vec<TaskTrackerToken> = self.outer.iter().map(TaskTracker::token).collect();
        let scoped = TASK.scope(
            TaskActivity::new(Arc::clone(&self.activity), label),
            async move {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => {}
                    () = task => {}
                }
            },
        );
        let task = async move {
            scoped.await;
            drop(counted);
        };
        self.tracker.spawn_on(task, &self.runtime);
    }

    /// Cancels every task of the group without waiting for them.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// Cancels every task of the group and waits until they have ended.
    pub async fn stop(&self) {
        self.cancel.cancel();
        self.tracker.close();
        self.tracker.wait().await;
    }
}

/// Runs `fut` in the current task, busy as `label` until it ends or parks:
/// for work that runs outside the core's tasks but must count, such as a
/// debug web bridge command. Inside it, [`parked`] and [`waiting_on_person`]
/// work as in a spawned task.
pub async fn busy_scope<F: Future>(
    activity: &Arc<Activity>,
    label: &'static str,
    fut: F,
) -> F::Output {
    TASK.scope(TaskActivity::new(Arc::clone(activity), label), fut)
        .await
}

/// Awaits `fut` without counting the current task as busy. Only for waits
/// whose wake-up carries its own activity: a [`Kick`](super::Kick), a timer,
/// a [`Tracked`](super::Tracked) item.
///
/// The task parks only once `fut` has been polled and is pending, so the
/// wait is registered (a timer with the clock, a waiter with its signal)
/// before the core can look idle: a test that advances the clock to the
/// next timer once the core is idle then always finds it.
pub async fn parked<F: Future>(fut: F) -> F::Output {
    let task = TASK.try_with(Arc::clone).ok();
    let mut unpark: Option<Unpark> = None;
    let mut fut = std::pin::pin!(fut);
    std::future::poll_fn(|cx| {
        let poll = fut.as_mut().poll(cx);
        if poll.is_pending()
            && unpark.is_none()
            && let Some(task) = &task
        {
            task.park();
            unpark = Some(Unpark(Arc::clone(task)));
        }
        poll
    })
    .await
}

struct Unpark(Arc<TaskActivity>);

impl Drop for Unpark {
    fn drop(&mut self) {
        self.0.unpark();
    }
}

/// From now until it ends, the current task waits on the person, not on the
/// core: a sign-in they may never finish in the browser.
pub fn waiting_on_person() {
    if TASK.try_with(|task| task.park()).is_err() {
        tracing::debug!("waiting_on_person outside a core task");
    }
}
