//! Writes the perf fixture or demo data into a database file.
//!
//!     cargo run --bin perf-fixture -- <db path> [perf|demo]

#![expect(
    clippy::print_stdout,
    reason = "A command-line tool reports on stdout."
)]

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: perf-fixture <db path> [perf|demo]"))?;
    let size = match args.next().as_deref() {
        Some("demo") => &ubermensch_lib::fixture::DEMO,
        _ => &ubermensch_lib::fixture::PERF,
    };
    let _ = std::fs::remove_file(&path);
    let started = std::time::Instant::now();
    ubermensch_lib::fixture::generate(
        std::path::Path::new(&path),
        size,
        &["alex@example.com", "sam@example.com"],
    )?;
    println!("wrote {path} in {:.1}s", started.elapsed().as_secs_f64());
    Ok(())
}
