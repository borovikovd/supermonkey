//! Local search and autocomplete against their SPEC budgets on the perf
//! fixture. The budgets are keydown to paint, so the query alone must fit well
//! inside them. Run with
//! `UBERMENSCH_PERF_DB=<path> cargo test --release --test perf_search -- --ignored --nocapture`.

#![expect(clippy::expect_used, reason = "Tests fail loudly on purpose.")]

use std::time::{Duration, Instant};

/// SPEC Performance: local search results for each keystroke.
const SEARCH_BUDGET: Duration = Duration::from_millis(100);
/// SPEC Performance: autocomplete suggestions for each keystroke.
const AUTOCOMPLETE_BUDGET: Duration = Duration::from_millis(50);

fn perf_db() -> rusqlite::Connection {
    let path = std::env::var("UBERMENSCH_PERF_DB")
        .expect("set UBERMENSCH_PERF_DB to a database written by the perf-fixture binary");
    rusqlite::Connection::open(path).expect("open the perf fixture")
}

#[test]
#[ignore = "needs UBERMENSCH_PERF_DB pointing at a perf fixture"]
fn local_search_per_keystroke() {
    let conn = perf_db();
    // `i` twice: the first runs on a cold cache.
    for q in [
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
    ] {
        let t = Instant::now();
        let ids = ubermensch_lib::subs::search::ids(&conn, 1, q, &[]).unwrap();
        let took = t.elapsed();
        println!(
            "{q:>18}: {:>4} hits in {:>6.1} ms",
            ids.len(),
            took.as_secs_f64() * 1000.0
        );
        assert!(took <= SEARCH_BUDGET, "{q:?} took {took:?}");
    }
    // The fixture's names, subjects and labels: an empty result would be fast for the wrong reason.
    for q in ["from:ada", "q3 planning", "label:Receipts", "subject:lunch"] {
        let ids = ubermensch_lib::subs::search::ids(&conn, 1, q, &[]).unwrap();
        assert!(!ids.is_empty(), "{q:?} found nothing on the perf fixture");
    }
}

#[test]
#[ignore = "needs UBERMENSCH_PERF_DB pointing at a perf fixture"]
fn autocomplete_per_keystroke() {
    let conn = perf_db();
    for q in ["a", "ad", "ada", "ada l", "gr", "hamilton", "x"] {
        let t = Instant::now();
        let s = ubermensch_lib::engine::contacts::suggest(&conn, 1, q, 6).unwrap();
        let took = t.elapsed();
        println!(
            "{q:>10}: {} suggestions in {:>5.1} ms",
            s.len(),
            took.as_secs_f64() * 1000.0
        );
        assert!(took <= AUTOCOMPLETE_BUDGET, "{q:?} took {took:?}");
    }
    let ada = ubermensch_lib::engine::contacts::suggest(&conn, 1, "ada", 6).unwrap();
    assert!(!ada.is_empty(), "no contact named Ada on the perf fixture");
}
