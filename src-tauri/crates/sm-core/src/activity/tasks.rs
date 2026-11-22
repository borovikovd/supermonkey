use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tokio_util::task::task_tracker::TaskTrackerToken;

use super::Activity;

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

#[expect(
    clippy::disallowed_types,
    reason = "`tokio::task_local!` keeps its value in a `RefCell`, which no code here touches"
)]
mod local {
    use super::{Arc, TaskActivity};

    tokio::task_local! {
        pub(super) static TASK: Arc<TaskActivity>;
    }
}
use local::TASK;

#[derive(Debug, Clone)]
pub struct Tasks {
    runtime: Handle,
    activity: Arc<Activity>,
    cancel: CancellationToken,
    tracker: TaskTracker,
    outer: Arc<[TaskTracker]>,
}

impl Tasks {
    pub fn new(runtime: Handle, activity: Arc<Activity>) -> Self {
        Self {
            runtime,
            activity,
            cancel: CancellationToken::new(),
            tracker: TaskTracker::new(),
            outer: Arc::new([]),
        }
    }

    #[must_use]
    pub(crate) fn child(&self) -> Self {
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

    #[must_use]
    pub const fn activity(&self) -> &Arc<Activity> {
        &self.activity
    }

    #[must_use]
    pub(crate) const fn token(&self) -> &CancellationToken {
        &self.cancel
    }

    pub fn spawn<F>(&self, label: &'static str, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if self.cancel.is_cancelled() {
            tracing::debug!(task = label, "not started: the group has stopped");
            return;
        }
        let cancel = self.cancel.clone();
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

    pub(crate) fn cancel(&self) {
        self.cancel.cancel();
    }

    pub async fn stop(&self) {
        self.cancel.cancel();
        self.tracker.close();
        self.tracker.wait().await;
    }
}

pub async fn busy_scope<F: Future>(
    activity: &Arc<Activity>,
    label: &'static str,
    fut: F,
) -> F::Output {
    TASK.scope(TaskActivity::new(Arc::clone(activity), label), fut)
        .await
}

pub(crate) async fn parked<F: Future>(fut: F) -> F::Output {
    let mut wait = Wait {
        task: TASK.try_with(Arc::clone).ok(),
        parked: false,
    };
    let mut fut = std::pin::pin!(fut);
    std::future::poll_fn(|cx| {
        let poll = fut.as_mut().poll(cx);
        if poll.is_pending() {
            wait.park();
        }
        poll
    })
    .await
}

struct Wait {
    task: Option<Arc<TaskActivity>>,
    parked: bool,
}

impl Wait {
    fn park(&mut self) {
        if !self.parked
            && let Some(task) = &self.task
        {
            task.park();
            self.parked = true;
        }
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        if self.parked
            && let Some(task) = &self.task
        {
            task.unpark();
        }
    }
}

pub fn waiting_on_person() {
    if TASK.try_with(|task| task.park()).is_err() {
        tracing::debug!("waiting_on_person outside a core task");
    }
}
