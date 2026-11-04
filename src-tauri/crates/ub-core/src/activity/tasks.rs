//! Spawned tasks: each belongs to a cancellation token, and each counts as
//! busy unless it is parked on a wait whose wake-up carries its own activity.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

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
/// `.await`; [`Tasks::stop`] also waits for them to end.
#[derive(Clone)]
pub struct Tasks {
    runtime: Handle,
    activity: Arc<Activity>,
    cancel: CancellationToken,
    tracker: TaskTracker,
}

impl Tasks {
    /// A root group on `runtime`, counting its tasks in `activity`.
    pub fn new(runtime: Handle, activity: Arc<Activity>) -> Self {
        Self {
            runtime,
            activity,
            cancel: CancellationToken::new(),
            tracker: TaskTracker::new(),
        }
    }

    /// A group inside this one: cancelled with it, or on its own.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            runtime: self.runtime.clone(),
            activity: Arc::clone(&self.activity),
            cancel: self.cancel.child_token(),
            tracker: TaskTracker::new(),
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
        self.tracker.spawn_on(scoped, &self.runtime);
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

/// Awaits `fut` without counting the current task as busy. Only for waits
/// whose wake-up carries its own activity: a [`Kick`](super::Kick), a timer,
/// a [`Tracked`](super::Tracked) item.
pub async fn parked<F: Future>(fut: F) -> F::Output {
    let _unpark = TASK.try_with(Arc::clone).ok().map(|task| {
        task.park();
        Unpark(task)
    });
    fut.await
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
