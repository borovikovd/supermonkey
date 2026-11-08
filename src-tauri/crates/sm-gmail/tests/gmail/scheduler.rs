//! Priority order, the in-flight cap, the quota bucket rules, and back-off,
//! on tokio's paused clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use sm_gmail::{GmailError, Outcome, Priority, QuotaConfig, Scheduler, Units, backoff, with_retry};
use tokio::time::{Instant, sleep};

const A: &str = "a@example.com";
const B: &str = "b@example.com";

/// Lets every spawned task run until it blocks.
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
    for level in [5, 3, 6, 1, 2, 4, 1] {
        let (account, order) = (account.clone(), Arc::clone(&order));
        let priority = Priority::try_from(level).unwrap();
        tasks.push(tokio::spawn(async move {
            account
                .run(priority, Units(1), async {
                    order.lock().push(level);
                })
                .await;
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
                account
                    .run(Priority::P2, Units(1), async {
                        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        assert!(account.in_flight() <= 8);
                        sleep(Duration::from_millis(10)).await;
                        running.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await;
            }));
        }
    }
    for task in tasks {
        task.await.unwrap();
    }
    // Eight per account, two accounts.
    assert_eq!(peak.load(Ordering::SeqCst), 16);
}

#[tokio::test(start_paused = true)]
async fn background_priorities_starve_below_the_reserve_but_priority_one_runs() {
    let scheduler = Scheduler::new(QuotaConfig::default());
    let account = scheduler.account(A);

    // Priorities 1-3 may take the bucket from 1000 down to 400.
    drop(account.acquire(Priority::P1, Units(600)).await);
    assert!((account.bucket_level() - 400.0).abs() < 1e-6);

    // Priority 4 waits until the bucket holds more than 500: 101 units at 80 a second.
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

    // Priority 1 is not held behind it and may drain the bucket.
    let p1 = Instant::now();
    drop(account.acquire(Priority::P1, Units(300)).await);
    assert_eq!(Instant::now(), p1);

    // Now the bucket holds about 100: priority 4 needs 401 more units, about 5 s.
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
    // 400 units take 5 s to refill; 10 would take 125 ms.
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
    // The reserve and refill alone would allow about 500 + 4800 units.
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
    // The slot frees and the waiter is granted, but it is cancelled before it runs.
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

    // A running request that is dropped frees the slot.
    let stuck = account.run(Priority::P2, Units(40), std::future::pending::<()>());
    assert!(
        tokio::time::timeout(Duration::from_secs(1), stuck)
            .await
            .is_err()
    );
    assert_eq!(account.in_flight(), 0);

    // A queued request that is dropped leaves the queue.
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
    // The head waits 5 s for 400 units; the cheap one behind it waits with it.
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

#[test]
fn backoff_doubles_to_a_minute_with_jitter() {
    for attempt in 0..10 {
        let base = Duration::from_secs(1 << attempt).min(Duration::from_mins(1));
        let wait = backoff(attempt, None);
        assert!(
            wait >= base.mul_f64(0.8) && wait <= base.mul_f64(1.2).min(Duration::from_mins(1)),
            "attempt {attempt}: {wait:?}"
        );
    }
    for attempt in [30, 32, 1000] {
        let wait = backoff(attempt, None);
        assert!(
            wait >= Duration::from_secs(48) && wait <= Duration::from_mins(1),
            "attempt {attempt}: {wait:?}"
        );
    }
    assert_eq!(
        backoff(3, Some(Duration::from_secs(5))),
        Duration::from_secs(5)
    );
}

#[tokio::test(start_paused = true)]
async fn with_retry_retries_transient_outcomes_only() {
    let calls = AtomicUsize::new(0);
    let result = with_retry(5, sleep, || async {
        match calls.fetch_add(1, Ordering::SeqCst) {
            0 => Err(GmailError::new(Outcome::ServerError { status: 503 })),
            1 => Err(GmailError::new(Outcome::RateLimited {
                retry_after: Some(Duration::from_secs(3)),
            })),
            2 => Err(GmailError::new(Outcome::NoResponse)),
            _ => Ok("done"),
        }
    })
    .await;
    assert_eq!(result.unwrap(), "done");
    assert_eq!(calls.load(Ordering::SeqCst), 4);

    let calls = AtomicUsize::new(0);
    let result: Result<(), _> = with_retry(5, sleep, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(GmailError::new(Outcome::NotFound))
    })
    .await;
    assert_eq!(result.unwrap_err().outcome(), &Outcome::NotFound);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let calls = AtomicUsize::new(0);
    let result: Result<(), _> = with_retry(3, sleep, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(GmailError::new(Outcome::ServerError { status: 500 }))
    })
    .await;
    assert_eq!(
        result.unwrap_err().outcome(),
        &Outcome::ServerError { status: 500 }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test(start_paused = true)]
async fn with_retry_waits_as_long_as_retry_after_says() {
    let calls = AtomicUsize::new(0);
    let start = Instant::now();
    let result = with_retry(2, sleep, || async {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(GmailError::new(Outcome::RateLimited {
                retry_after: Some(Duration::from_secs(30)),
            }))
        } else {
            Ok("done")
        }
    })
    .await;
    assert_eq!(result.unwrap(), "done");
    assert_eq!(Instant::now() - start, Duration::from_secs(30));
}
