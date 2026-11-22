use std::time::{Duration, Instant};

use sm_domain::{AccountId, SubId};
use sm_ipc::{Action, ChannelMessage, SubKey, SubUpdate};

use super::Harness;
use crate::Config;

const SEARCH_BUDGET: Duration = Duration::from_millis(100);
const AUTOCOMPLETE_BUDGET: Duration = Duration::from_millis(50);
const ACCOUNT: AccountId = AccountId(1);

async fn on_fixture() -> Harness {
    let fixture = std::env::var_os("SUPERMONKEY_PERF_DB")
        .expect("set SUPERMONKEY_PERF_DB to a database written by the perf-fixture binary");
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join(Config::DB_FILE);
    std::fs::copy(&fixture, &copy).unwrap();
    drop(std::fs::read(&copy).unwrap());
    Harness::start_in(dir).await
}

const SEARCHES: [&str; 18] = [
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
    "in:sent",
    "is:unread",
    "has:attachment",
    "-in:inbox",
    "after:2020/01/01",
    "-newsletter",
    "invoice -newsletter",
];

fn first_load_len(messages: &[ChannelMessage], sub: &SubId) -> usize {
    messages
        .iter()
        .flat_map(|m| &m.updates)
        .find_map(|u| match u {
            SubUpdate::List { sub: s, ids, .. } if s == sub => ids.as_ref().map(Vec::len),
            SubUpdate::List { .. }
            | SubUpdate::Conversation { .. }
            | SubUpdate::Composer { .. }
            | SubUpdate::Account { .. }
            | SubUpdate::App { .. } => None,
        })
        .expect("the search's first load")
}

#[tokio::test]
#[ignore = "needs SUPERMONKEY_PERF_DB pointing at a perf fixture"]
async fn perf_local_search_per_keystroke() {
    let mut h = on_fixture().await;
    h.take();
    let mut slowest = Duration::ZERO;
    for (n, query) in SEARCHES.into_iter().enumerate() {
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
        let hits = first_load_len(&h.take(), &sub);
        println!(
            "{query:>18}: {hits:>4} hits in {:>6.1} ms",
            took.as_secs_f64() * 1000.0
        );
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
#[ignore = "needs SUPERMONKEY_PERF_DB pointing at a perf fixture"]
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
