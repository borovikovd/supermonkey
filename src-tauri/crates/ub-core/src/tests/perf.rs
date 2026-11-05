//! Local search and autocomplete against their SPEC Performance budgets on
//! the perf fixture. The budgets are keydown to paint, so the core's part
//! must fit well inside them. Ignored by default; run with
//!
//! ```sh
//! cargo run --features fake --bin perf-fixture -- /tmp/perf.db
//! UBERMENSCH_PERF_DB=/tmp/perf.db cargo test -p ub-core --release --lib perf -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use ub_domain::{AccountId, SubId};
use ub_ipc::{Action, SubKey, SubUpdate};

use super::Harness;
use crate::Config;

/// SPEC Performance: local search results for each keystroke.
const SEARCH_BUDGET: Duration = Duration::from_millis(100);
/// SPEC Performance: autocomplete suggestions for each keystroke.
const AUTOCOMPLETE_BUDGET: Duration = Duration::from_millis(50);
/// The perf fixture's first account.
const ACCOUNT: AccountId = AccountId(1);

/// A core on a copy of the perf fixture, so the fixture stays as written.
/// The copy is read through once, so the system has it cached as it has a
/// running app's database: reading a cold disk is part of a cold launch
/// (its own budget), not of a keystroke.
async fn on_fixture() -> Harness {
    let fixture = std::env::var("UBERMENSCH_PERF_DB")
        .expect("set UBERMENSCH_PERF_DB to a database written by the perf-fixture binary");
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join(Config::DB_FILE);
    std::fs::copy(&fixture, &copy).unwrap();
    drop(std::fs::read(&copy).unwrap());
    Harness::start_in(dir).await
}

#[tokio::test]
#[ignore = "needs UBERMENSCH_PERF_DB pointing at a perf fixture"]
async fn perf_local_search_per_keystroke() {
    let mut h = on_fixture().await;
    h.take();
    // `i` twice: the first runs on a cold cache.
    let queries = [
        "i",
        "i",
        "a",
        "in",
        "invoice",
        "from:ada",
        "q3 planning",
        "budget OR travel",
        "label:Receipts",
        "is:list",
        "subject:lunch",
    ];
    let mut slowest = Duration::ZERO;
    for (n, query) in queries.into_iter().enumerate() {
        let sub = SubId(format!("search-{n}"));
        let started = Instant::now();
        h.act(vec![Action::Subscribe {
            sub: sub.clone(),
            key: SubKey::Search {
                account: ACCOUNT,
                query: query.to_owned(),
            },
        }])
        .await;
        let took = started.elapsed();
        let hits = h
            .take()
            .iter()
            .flat_map(|m| &m.updates)
            .find_map(|u| match u {
                SubUpdate::List { sub: s, ids, .. } if *s == sub => ids.as_ref().map(Vec::len),
                _ => None,
            })
            .expect("the search's first load");
        println!(
            "{query:>18}: {hits:>4} hits in {:>6.1} ms",
            took.as_secs_f64() * 1000.0
        );
        // The fixture has these names, subjects, and labels: an empty
        // result would be fast for the wrong reason.
        if ["from:ada", "q3 planning", "label:Receipts", "subject:lunch"].contains(&query) {
            assert!(hits > 0, "{query:?} found nothing on the perf fixture");
        }
        h.act(vec![Action::Unsubscribe { sub }]).await;
        slowest = slowest.max(took);
    }
    assert!(
        slowest <= SEARCH_BUDGET,
        "the slowest search took {slowest:?}"
    );
}

#[tokio::test]
#[ignore = "needs UBERMENSCH_PERF_DB pointing at a perf fixture"]
async fn perf_autocomplete_per_keystroke() {
    let h = on_fixture().await;
    let mut slowest = Duration::ZERO;
    for query in ["a", "ad", "ada", "ada l", "gr", "hamilton", "x"] {
        let started = Instant::now();
        let suggestions = h
            .app
            .queries()
            .autocomplete(ACCOUNT, query.to_owned())
            .await
            .unwrap();
        let took = started.elapsed();
        println!(
            "{query:>10}: {} suggestions in {:>5.1} ms",
            suggestions.len(),
            took.as_secs_f64() * 1000.0
        );
        if query == "ada" {
            assert!(
                !suggestions.is_empty(),
                "no contact named Ada on the perf fixture"
            );
        }
        slowest = slowest.max(took);
    }
    assert!(
        slowest <= AUTOCOMPLETE_BUDGET,
        "the slowest autocomplete took {slowest:?}"
    );
}
