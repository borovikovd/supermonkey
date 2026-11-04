//! App time and activity, ported from the old `clock.rs` tests: tasks are
//! busy until they park, kicks and fired timers carry their own activity,
//! and the manual clock fires each due timer once.

use std::sync::Arc;
use std::time::Duration;

use ub_domain::Millis;

use crate::activity::{Activity, Kick, Tasks};
use crate::clock::{Clock, ManualClock, SystemClock, sleep};

fn busy(activity: &Activity, label: &str) -> usize {
    activity.snapshot().get(label).copied().unwrap_or(0)
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

fn tasks(activity: &Arc<Activity>) -> Tasks {
    Tasks::new(tokio::runtime::Handle::current(), Arc::clone(activity))
}

#[tokio::test]
async fn a_task_is_busy_until_it_parks_and_again_once_kicked() {
    let activity = Activity::new();
    let kick = Arc::new(Kick::new(Arc::clone(&activity)));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let (k, a) = (Arc::clone(&kick), Arc::clone(&activity));
    let group = tasks(&activity);
    group.spawn("test-kick", async move {
        k.wait(&SystemClock, Duration::from_secs(3600)).await;
        tx.send(busy(&a, "test-kick")).unwrap();
    });
    until("the task parks", || {
        kick.waiters() == 1 && busy(&activity, "test-kick") == 0
    })
    .await;
    kick.kick();
    // Busy from the kick until the task has resumed and finished.
    assert!(busy(&activity, "test-kick") + busy(&activity, "kick") >= 1);
    assert_eq!(rx.recv().await, Some(1), "busy again once resumed");
    group.stop().await;
    assert_eq!(busy(&activity, "test-kick"), 0);
    assert_eq!(busy(&activity, "kick"), 0, "a taken kick stays busy");
    assert!(activity.is_idle());
}

#[tokio::test]
async fn a_kick_with_no_waiter_is_kept_busy_for_the_next_wait() {
    let activity = Activity::new();
    let kick = Kick::new(Arc::clone(&activity));
    kick.kick();
    assert_eq!(busy(&activity, crate::activity::DEFERRED_KICK), 1);
    tokio::time::timeout(
        Duration::from_secs(5),
        kick.wait(&SystemClock, Duration::from_secs(3600)),
    )
    .await
    .expect("the kept kick ends the wait at once");
    assert!(activity.is_idle());
}

#[tokio::test]
async fn a_closed_kick_wakes_its_waiter_and_every_later_wait() {
    let activity = Activity::new();
    let kick = Arc::new(Kick::new(Arc::clone(&activity)));
    let k = Arc::clone(&kick);
    let waiter = tokio::spawn(async move { k.wait(&SystemClock, Duration::from_secs(3600)).await });
    until("the waiter waits", || kick.waiters() == 1).await;
    kick.close();
    waiter.await.unwrap();
    // A loop that checked before the close and waits after it: no hour-long wait.
    tokio::time::timeout(
        Duration::from_secs(5),
        kick.wait(&SystemClock, Duration::from_secs(3600)),
    )
    .await
    .expect("a wait after close returns at once");
}

#[tokio::test]
async fn advance_fires_due_sleeps_once_and_jump_fires_none() {
    let activity = Activity::new();
    let clock = Arc::new(ManualClock::new(Arc::clone(&activity), Millis(1_000)));
    let started = |ms: u64| {
        let c = Arc::clone(&clock);
        tokio::spawn(async move { sleep(c.as_ref(), Duration::from_millis(ms)).await })
    };
    let (short, long) = (started(500), started(2_000));
    until("both sleeps register", || clock.sleepers() == 2).await;
    assert_eq!(clock.next_due_within(400), None);
    assert_eq!(clock.next_due_within(500), Some(500));
    clock.jump(10_000);
    assert_eq!(clock.now(), Millis(11_000));
    assert_eq!(clock.advance(499), 0);
    assert_eq!(clock.advance(1), 1);
    short.await.unwrap();
    assert!(!long.is_finished());
    assert_eq!(clock.advance(5_000), 1);
    long.await.unwrap();
    assert_eq!(clock.now(), Millis(16_500));
    assert!(
        activity.is_idle(),
        "a fired timer is busy only until its task resumes"
    );
}

#[tokio::test]
async fn a_dropped_sleep_leaves_no_deadline_behind() {
    let activity = Activity::new();
    let clock = Arc::new(ManualClock::new(Arc::clone(&activity), Millis(0)));
    let c = Arc::clone(&clock);
    let sleeping = tokio::spawn(async move { sleep(c.as_ref(), Duration::from_millis(300)).await });
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
    let activity = Activity::new();
    let item = activity.track("test-item", 7);
    assert_eq!(busy(&activity, "test-item"), 1);
    let (value, done) = item.into_parts();
    assert_eq!(value, 7);
    assert_eq!(busy(&activity, "test-item"), 1);
    drop(done);
    assert_eq!(busy(&activity, "test-item"), 0);
}

#[tokio::test]
async fn a_cancelled_group_ends_its_tasks_and_starts_no_more() {
    let activity = Activity::new();
    let group = tasks(&activity);
    group.spawn("forever", std::future::pending());
    until("the task counts", || busy(&activity, "forever") == 1).await;
    group.stop().await;
    assert!(activity.is_idle());
    group.spawn("late", async {});
    assert!(activity.is_idle(), "nothing starts after the group stopped");
}
