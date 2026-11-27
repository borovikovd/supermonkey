use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use chrono::{TimeZone as _, Utc};
use supermonkey_lib::domain::contacts::{contact_matches, is_excluded, rank};
use supermonkey_lib::domain::query::Query;
use supermonkey_lib::domain::{AccountId, Millis};
use supermonkey_lib::store::ReadPool;

const SEARCH_BUDGET_MS: u128 = 100;
const AUTOCOMPLETE_BUDGET_MS: u128 = 50;

#[test]
#[ignore = "run explicitly against a fixture built by perf-fixture via SUPERMONKEY_PERF_DB"]
fn search_and_autocomplete_meet_latency_budgets() {
    let raw = std::env::var_os("SUPERMONKEY_PERF_DB")
        .expect("set SUPERMONKEY_PERF_DB to a perf-fixture database path");
    let path = PathBuf::from(raw);
    let pool = ReadPool::open(&path, NonZeroUsize::new(2).unwrap()).unwrap();
    #[expect(
        clippy::redundant_closure_for_method_calls,
        reason = "Db carries a lifetime so a closure is needed for ReadPool::with HRTB"
    )]
    let accounts = pool.with(|db| db.accounts()).unwrap();
    assert!(
        !accounts.is_empty(),
        "perf database must contain at least one account"
    );
    let first = accounts.into_iter().next().unwrap();
    let acct: AccountId = first.id;
    let identity = first.into_identity();
    let now = Utc.with_ymd_and_hms(2026, 11, 2, 15, 0, 0).unwrap();
    let now_ms = Millis(now.timestamp_millis());

    for raw_q in [
        "quarterly",
        "from:alice",
        "has:attachment invoice",
        "is:unread",
    ] {
        let parsed = Query::parse_search(raw_q, &now);
        let t0 = Instant::now();
        let hits = pool
            .with(|db| db.search_threads(acct, &parsed, &[]))
            .unwrap();
        let elapsed = t0.elapsed().as_millis();
        assert!(
            elapsed <= SEARCH_BUDGET_MS,
            "search {raw_q:?} took {elapsed}ms ({} hits), budget is {SEARCH_BUDGET_MS}ms",
            hits.len()
        );
    }

    for q in ["al", "car", "eng", "test"] {
        let t0 = Instant::now();
        let count = pool
            .with(|db| {
                let cands = db.contact_candidates(acct, q)?;
                let mut scored = Vec::new();
                for s in cands {
                    if !is_excluded(&s, &identity)
                        && let Some(span) = contact_matches(&s.name, &s.email, q)
                    {
                        scored.push((rank(&s, now_ms), s, span));
                    }
                }
                scored.sort_by(|a, b| a.0.cmp_desc(b.0));
                scored.truncate(8);
                Ok::<_, supermonkey_lib::store::StoreError>(scored.len())
            })
            .unwrap();
        let elapsed = t0.elapsed().as_millis();
        assert!(
            elapsed <= AUTOCOMPLETE_BUDGET_MS,
            "autocomplete {q:?} took {elapsed}ms ({count} results), budget is {AUTOCOMPLETE_BUDGET_MS}ms"
        );
    }
}
