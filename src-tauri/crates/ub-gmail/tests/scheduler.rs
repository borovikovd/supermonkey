//! Priority order, the in-flight cap, and the quota bucket rules, on tokio's paused clock.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::{Instant, sleep};
use ub_gmail::{GmailError, Outcome, Priority, QuotaConfig, Scheduler, backoff, with_retry};

const A: &str = "a@example.com";

/// Lets every spawned task run until it blocks.
async fn settle() {
    sleep(Duration::from_millis(1)).await;
}

#[tokio::test(start_paused = true)]
async fn queued_requests_run_in_priority_order() {
    let scheduler = Scheduler::new(QuotaConfig {
        max_in_flight: 1,
        ..QuotaConfig::default()
    });
    let account = scheduler.account(A);
    let order = Arc::new(Mutex::new(Vec::new()));

    let blocker = account.acquire(Priority::P1, 1).await;
    let mut tasks = Vec::new();
    for level in [5, 3, 6, 1, 2, 4, 1] {
        let (account, order) = (account.clone(), order.clone());
        tasks.push(tokio::spawn(async move {
            account
                .run(Priority::new(level), 1, async {
                    order.lock().unwrap().push(level);
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
    assert_eq!(*order.lock().unwrap(), vec![1, 1, 2, 3, 4, 5, 6]);
}

#[tokio::test(start_paused = true)]
async fn at_most_eight_requests_run_at_once_per_account() {
    let scheduler = Scheduler::new(QuotaConfig::default());
    let running = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for account in [A, "b@example.com"] {
        for _ in 0..20 {
            let account = scheduler.account(account);
            let (running, peak) = (running.clone(), peak.clone());
            tasks.push(tokio::spawn(async move {
                account
                    .run(Priority::P2, 1, async {
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
    drop(account.acquire(Priority::P1, 600).await);
    assert!((account.bucket_level() - 400.0).abs() < 1e-6);

    // Priority 4 waits until the bucket holds more than 500: 101 units at 80 a second.
    let start = Instant::now();
    let background = tokio::spawn({
        let account = account.clone();
        async move {
            drop(account.acquire(Priority::P4, 10).await);
            Instant::now()
        }
    });
    settle().await;
    assert_eq!(account.queued(), 1);

    // Priority 1 is not held behind it and may drain the bucket.
    let p1 = Instant::now();
    drop(account.acquire(Priority::P1, 300).await);
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
    drop(account.acquire(Priority::P1, 1000).await);
    let start = Instant::now();
    drop(account.acquire(Priority::P1, 120).await);
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
    drop(account.acquire(Priority::P1, 1000).await);
    let order = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = Vec::new();
    // 400 units take 5 s to refill; 10 would take 125 ms.
    for cost in [400, 10] {
        let (account, order) = (account.clone(), order.clone());
        tasks.push(tokio::spawn(async move {
            drop(account.acquire(Priority::P1, cost).await);
            order.lock().unwrap().push(cost);
        }));
        settle().await;
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(*order.lock().unwrap(), vec![400, 10]);
}

#[tokio::test(start_paused = true)]
async fn background_spending_is_capped_per_minute() {
    let scheduler = Scheduler::new(QuotaConfig::default());
    let account = scheduler.account(A);
    let start = Instant::now();
    let mut spent_in_first_minute = 0;
    loop {
        drop(account.acquire(Priority::P5, 100).await);
        if Instant::now() - start >= Duration::from_secs(60) {
            break;
        }
        spent_in_first_minute += 100;
    }
    // The reserve and refill alone would allow about 500 + 4800 units.
    assert_eq!(spent_in_first_minute, 4000);
}

#[tokio::test(start_paused = true)]
async fn dropping_a_queued_or_running_request_frees_its_place() {
    let scheduler = Scheduler::new(QuotaConfig {
        max_in_flight: 1,
        ..QuotaConfig::default()
    });
    let account = scheduler.account(A);

    // A running request that is dropped frees the slot.
    let stuck = account.run(Priority::P2, 40, std::future::pending::<()>());
    assert!(
        tokio::time::timeout(Duration::from_secs(1), stuck)
            .await
            .is_err()
    );
    assert_eq!(account.in_flight(), 0);

    // A queued request that is dropped leaves the queue.
    let holder = account.acquire(Priority::P2, 1).await;
    let queued = tokio::spawn({
        let account = account.clone();
        async move { drop(account.acquire(Priority::P2, 1).await) }
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
async fn accounts_do_not_wait_for_each_other() {
    let scheduler = Scheduler::new(QuotaConfig {
        max_in_flight: 1,
        ..QuotaConfig::default()
    });
    scheduler.set_on_screen(Some("b@example.com"));
    let a = scheduler.account(A);
    let _held = a.acquire(Priority::P1, 1000).await;
    let b = scheduler.account("b@example.com");
    let start = Instant::now();
    drop(b.acquire(Priority::P6, 100).await);
    assert_eq!(Instant::now(), start);
}

#[test]
fn backoff_doubles_to_a_minute_with_jitter() {
    for attempt in 0..10 {
        let base = Duration::from_secs(1 << attempt).min(Duration::from_secs(60));
        let wait = backoff(attempt, None);
        assert!(
            wait >= base.mul_f64(0.8) && wait <= base.mul_f64(1.2).min(Duration::from_secs(60))
        );
    }
    for attempt in [30, 32, 1000] {
        let wait = backoff(attempt, None);
        assert!(
            wait >= Duration::from_secs(48) && wait <= Duration::from_secs(60),
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
    let result = with_retry(5, || async {
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
    let result: Result<(), _> = with_retry(5, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(GmailError::new(Outcome::NotFound))
    })
    .await;
    assert_eq!(result.unwrap_err().outcome, Outcome::NotFound);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let calls = AtomicUsize::new(0);
    let result: Result<(), _> = with_retry(3, || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(GmailError::new(Outcome::ServerError { status: 500 }))
    })
    .await;
    assert_eq!(
        result.unwrap_err().outcome,
        Outcome::ServerError { status: 500 }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test(start_paused = true)]
async fn with_retry_waits_as_long_as_retry_after_says() {
    let calls = AtomicUsize::new(0);
    let start = Instant::now();
    let result = with_retry(2, || async {
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
