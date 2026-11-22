use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use sm_gmail::{Priority, QuotaConfig, Scheduler, Units};
use tokio::time::{Instant, sleep};

const A: &str = "a@example.com";
const B: &str = "b@example.com";

async fn settle() {
    sleep(Duration::from_millis(1)).await;
}

fn one_at_a_time() -> QuotaConfig {
    QuotaConfig {
        max_in_flight: 1,
        ..QuotaConfig::default()
    }
}

#[tokio::test(start_paused = true)]
async fn queued_requests_run_in_priority_order() {
    let scheduler = Scheduler::new(one_at_a_time());
    let account = scheduler.account(A);
    let order = Arc::new(Mutex::new(Vec::new()));

    let blocker = account.acquire(Priority::P1, Units(1)).await;
    let mut tasks = Vec::new();
    for (level, priority) in [
        (5, Priority::P5),
        (3, Priority::P3),
        (6, Priority::P6),
        (1, Priority::P1),
        (2, Priority::P2),
        (4, Priority::P4),
        (1, Priority::P1),
    ] {
        let (account, order) = (account.clone(), Arc::clone(&order));
        tasks.push(tokio::spawn(async move {
            let _permit = account.acquire(priority, Units(1)).await;
            order.lock().push(level);
        }));
        settle().await;
    }
    assert_eq!(account.queued(), 7);
    drop(blocker);
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(*order.lock(), vec![1, 1, 2, 3, 4, 5, 6]);
}

#[tokio::test(start_paused = true)]
async fn a_forgotten_account_starts_afresh() {
    let scheduler = Scheduler::new(one_at_a_time());
    let held = scheduler.account(A).acquire(Priority::P1, Units(1)).await;
    scheduler.forget(A);
    let fresh = tokio::time::timeout(
        Duration::from_secs(1),
        scheduler.account(A).acquire(Priority::P1, Units(1)),
    )
    .await;
    assert!(fresh.is_ok(), "the removed account's slot is not shared");
    drop(held);
}

#[tokio::test(start_paused = true)]
async fn at_most_eight_requests_run_at_once_per_account() {
    let scheduler = Scheduler::new(QuotaConfig::default());
    let running = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for email in [A, B] {
        for _ in 0..20 {
            let account = scheduler.account(email);
            let (running, peak) = (Arc::clone(&running), Arc::clone(&peak));
            tasks.push(tokio::spawn(async move {
                let _permit = account.acquire(Priority::P2, Units(1)).await;
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                assert!(account.in_flight() <= 8);
                sleep(Duration::from_millis(10)).await;
                running.fetch_sub(1, Ordering::SeqCst);
            }));
        }
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(peak.load(Ordering::SeqCst), 16);
}

#[tokio::test(start_paused = true)]
async fn background_priorities_starve_below_the_reserve_but_priority_one_runs() {
    let scheduler = Scheduler::new(QuotaConfig::default());
    let account = scheduler.account(A);

    drop(account.acquire(Priority::P1, Units(600)).await);
    assert!((account.bucket_level() - 400.0).abs() < 1e-6);

    let start = Instant::now();
    let background = tokio::spawn({
        let account = account.clone();
        async move {
            drop(account.acquire(Priority::P4, Units(10)).await);
            Instant::now()
        }
    });
    settle().await;
    assert_eq!(account.queued(), 1);

    let p1 = Instant::now();
    drop(account.acquire(Priority::P1, Units(300)).await);
    assert_eq!(Instant::now(), p1);

    let ran_at = background.await.unwrap();
    let waited = ran_at - start;
    let expected = Duration::from_secs_f64(401.0 / 80.0);
    assert!(
        waited >= expected.checked_sub(Duration::from_millis(5)).unwrap()
            && waited < expected + Duration::from_millis(50),
        "{waited:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn priority_one_waits_for_refill_once_the_bucket_is_empty() {
    let scheduler = Scheduler::new(QuotaConfig::default());
    let account = scheduler.account(A);
    drop(account.acquire(Priority::P1, Units(1000)).await);
    let start = Instant::now();
    drop(account.acquire(Priority::P1, Units(120)).await);
    let waited = Instant::now() - start;
    assert!(
        waited >= Duration::from_millis(1500) && waited < Duration::from_millis(1550),
        "{waited:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_cheap_request_does_not_overtake_an_earlier_one_waiting_for_budget() {
    let scheduler = Scheduler::new(QuotaConfig::default());
    let account = scheduler.account(A);
    drop(account.acquire(Priority::P1, Units(1000)).await);
    let order = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = Vec::new();
    for cost in [400, 10] {
        let (account, order) = (account.clone(), Arc::clone(&order));
        tasks.push(tokio::spawn(async move {
            drop(account.acquire(Priority::P1, Units(cost)).await);
            order.lock().push(cost);
        }));
        settle().await;
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(*order.lock(), vec![400, 10]);
}

#[tokio::test(start_paused = true)]
async fn background_spending_is_capped_per_minute() {
    let scheduler = Scheduler::new(QuotaConfig::default());
    let account = scheduler.account(A);
    let start = Instant::now();
    let mut spent_in_first_minute = 0;
    loop {
        drop(account.acquire(Priority::P5, Units(100)).await);
        if Instant::now() - start >= Duration::from_secs(60) {
            break;
        }
        spent_in_first_minute += 100;
    }
    assert_eq!(spent_in_first_minute, 4000);
}

#[tokio::test(start_paused = true)]
async fn a_background_request_dropped_once_granted_gives_back_its_minute_budget() {
    let scheduler = Scheduler::new(QuotaConfig {
        background_per_minute: Units(100),
        ..one_at_a_time()
    });
    let account = scheduler.account(A);
    let holder = account.acquire(Priority::P2, Units(1)).await;
    let queued = tokio::spawn({
        let account = account.clone();
        async move { drop(account.acquire(Priority::P5, Units(100)).await) }
    });
    settle().await;
    assert_eq!(account.queued(), 1);
    drop(holder);
    queued.abort();
    settle().await;
    assert_eq!(account.in_flight(), 0);

    let start = Instant::now();
    drop(account.acquire(Priority::P5, Units(100)).await);
    assert!(
        Instant::now() - start < Duration::from_secs(1),
        "the cancelled request still counted against the background cap"
    );
}

#[tokio::test(start_paused = true)]
async fn dropping_a_queued_or_running_request_frees_its_place() {
    let scheduler = Scheduler::new(one_at_a_time());
    let account = scheduler.account(A);

    let stuck = async {
        let _permit = account.acquire(Priority::P2, Units(40)).await;
        std::future::pending::<()>().await;
    };
    assert!(
        tokio::time::timeout(Duration::from_secs(1), stuck)
            .await
            .is_err()
    );
    assert_eq!(account.in_flight(), 0);

    let holder = account.acquire(Priority::P2, Units(1)).await;
    let queued = tokio::spawn({
        let account = account.clone();
        async move { drop(account.acquire(Priority::P2, Units(1)).await) }
    });
    settle().await;
    assert_eq!(account.queued(), 1);
    queued.abort();
    settle().await;
    assert_eq!(account.queued(), 0);
    drop(holder);
    assert_eq!(account.in_flight(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_dropped_waiter_at_the_head_lets_the_next_one_run() {
    let scheduler = Scheduler::new(QuotaConfig::default());
    let account = scheduler.account(A);
    drop(account.acquire(Priority::P1, Units(1000)).await);
    let head = tokio::spawn({
        let account = account.clone();
        async move { drop(account.acquire(Priority::P1, Units(400)).await) }
    });
    settle().await;
    let start = Instant::now();
    let cheap = tokio::spawn({
        let account = account.clone();
        async move {
            drop(account.acquire(Priority::P1, Units(10)).await);
            Instant::now()
        }
    });
    settle().await;
    head.abort();
    let ran_at = tokio::time::timeout(Duration::from_secs(10), cheap)
        .await
        .expect("the waiter behind a dropped head never ran")
        .unwrap();
    assert!(
        ran_at - start < Duration::from_millis(200),
        "{:?}",
        ran_at - start
    );
}

#[tokio::test(start_paused = true)]
async fn accounts_do_not_wait_for_each_other() {
    let scheduler = Scheduler::new(one_at_a_time());
    let a = scheduler.account(A);
    let _held = a.acquire(Priority::P1, Units(1000)).await;
    let b = scheduler.account(B);
    let start = Instant::now();
    drop(b.acquire(Priority::P6, Units(100)).await);
    assert_eq!(Instant::now(), start);
}
