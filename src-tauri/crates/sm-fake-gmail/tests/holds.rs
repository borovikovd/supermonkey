//! Holds against releases that race them: a release must never strand a request.

#![expect(clippy::expect_used, reason = "Tests fail loudly on purpose.")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use sm_fake_gmail::{FakeGmail, Fault, Matcher, Release};
use tokio::time::timeout;

/// Long enough for any unparked request on a loaded machine.
const ANSWER_LIMIT: Duration = Duration::from_secs(5);
/// On the code this guards against, the race strands a request every hundred rounds or so.
const ROUNDS: usize = 2000;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_racing_a_hold_never_strands_the_request() {
    let fake = Arc::new(FakeGmail::start());
    let http = reqwest::Client::new();
    let url = format!("{}/gmail/v1/users/me/threads", fake.base_url());
    // Contend for the held list, so a handler that has chosen a hold sometimes waits to
    // park on it while the release gets there first.
    let stop = Arc::new(AtomicBool::new(false));
    let poller = {
        let (fake, stop) = (Arc::clone(&fake), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::hint::black_box(fake.held());
            }
        })
    };
    let mut stranded = None;
    for round in 0..ROUNDS {
        let id = fake.inject(Matcher::api("threads.list"), Fault::Hold);
        let before = fake.in_flight();
        let request = tokio::spawn(http.get(&url).send());
        // The handler counts itself in flight just before it looks for a fault: release
        // right then, to land between choosing the hold and parking on it.
        let arrival = Instant::now() + ANSWER_LIMIT;
        while fake.in_flight() == before {
            assert!(
                Instant::now() < arrival,
                "round {round}: the request never arrived"
            );
            std::hint::spin_loop();
        }
        fake.release(&Release::Hold(id), None);
        if timeout(ANSWER_LIMIT, request).await.is_err() {
            stranded = Some(round);
            break;
        }
    }
    stop.store(true, Ordering::Relaxed);
    poller.join().expect("poller");
    assert_eq!(
        stranded, None,
        "a release that raced the hold left the request parked"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_while_a_held_answer_is_worked_on_lets_the_answer_through() {
    let fake = FakeGmail::start();
    let user = fake.add_user("me@example.com");
    let http = reqwest::Client::new();
    let token = access_token(&http, &fake, user.refresh_token()).await;
    let id = fake.inject(Matcher::api("threads.list"), Fault::HoldAnswer);

    // A request reads the clock twice: on arrival, before any fault is chosen, and in
    // `threads.list` itself, mid-work. Stop it at the second until the release is done.
    let working = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let reads = AtomicUsize::new(0);
    let (w, r) = (Arc::clone(&working), Arc::clone(&resume));
    fake.set_clock(move || {
        if reads.fetch_add(1, Ordering::SeqCst) == 1 {
            w.wait();
            r.wait();
        }
        1_700_000_000_000
    });

    let request = tokio::spawn(
        http.get(format!("{}/gmail/v1/users/me/threads", fake.base_url()))
            .bearer_auth(token)
            .send(),
    );
    wait(&working).await;
    // Not parked yet: only a finished answer counts as held.
    let held_while_working = fake.held();
    let released = fake.release(&Release::Hold(id), None);
    wait(&resume).await;

    assert_eq!(held_while_working, 0);
    assert_eq!(
        released, 1,
        "the release covers the answer still being worked on"
    );
    let response = timeout(ANSWER_LIMIT, request)
        .await
        .expect("the answer is not parked after its release")
        .expect("request task")
        .expect("response");
    assert_eq!(response.status(), 200);
    assert_eq!(fake.held(), 0);
    assert!(fake.requests().iter().all(|r| !r.held));
}

async fn wait(barrier: &Arc<Barrier>) {
    let barrier = Arc::clone(barrier);
    tokio::task::spawn_blocking(move || barrier.wait())
        .await
        .expect("barrier wait");
}

async fn access_token(http: &reqwest::Client, fake: &FakeGmail, refresh: &str) -> String {
    let body = http
        .post(fake.token_url())
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!("grant_type=refresh_token&refresh_token={refresh}"))
        .send()
        .await
        .expect("token request")
        .text()
        .await
        .expect("token body");
    let json: serde_json::Value = serde_json::from_str(&body).expect("token json");
    json["access_token"]
        .as_str()
        .expect("access token")
        .to_owned()
}
