//! Local search timing on the perf fixture. Run with
//! `UBERMENSCH_PERF_DB=<path> cargo test --release --test perf_search -- --ignored --nocapture`.

#[test]
#[ignore]
fn local_search_per_keystroke() {
    let Ok(path) = std::env::var("UBERMENSCH_PERF_DB") else {
        return;
    };
    let conn = rusqlite::Connection::open(path).unwrap();
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
        let t = std::time::Instant::now();
        let ids = ubermensch_lib::subs::search::ids(&conn, 1, q, &[]).unwrap();
        println!(
            "{q:>18}: {:>4} hits in {:>6.1} ms",
            ids.len(),
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}

#[test]
#[ignore]
fn autocomplete_per_keystroke() {
    let Ok(path) = std::env::var("UBERMENSCH_PERF_DB") else {
        return;
    };
    let conn = rusqlite::Connection::open(path).unwrap();
    for q in ["a", "ad", "ada", "ada l", "gr", "hamilton", "x"] {
        let t = std::time::Instant::now();
        let s = ubermensch_lib::engine::contacts::suggest(&conn, 1, q, 6).unwrap();
        println!(
            "{q:>10}: {} suggestions in {:>5.1} ms",
            s.len(),
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}
