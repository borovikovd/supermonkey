//! Writes the perf fixture (SPEC Performance) or demo-sized data into a new
//! database file, through the store, as sync would leave it:
//!
//! `cargo run --features fake --bin perf-fixture -- <db path> [perf|demo]`
//!
//! Point the app at it with `SUPERMONKEY_DB=<db path>`, or the search bench
//! with `SUPERMONKEY_PERF_DB=<db path>`.

use std::path::Path;
use std::time::Instant;

use anyhow::Context as _;
use sm_core::clock::{Clock as _, SystemClock};
use sm_testkit::fixture::{DEMO, PERF, generate};

/// The fixture's accounts.
const ACCOUNTS: [&str; 2] = ["alex@example.com", "sam@example.com"];

fn main() -> anyhow::Result<()> {
    supermonkey_lib::init_logging();
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .context("usage: perf-fixture <db path> [perf|demo]")?;
    let size = match args.next().as_deref() {
        None | Some("perf") => PERF,
        Some("demo") => DEMO,
        Some(other) => anyhow::bail!("unknown size {other}: perf or demo"),
    };
    let path = Path::new(&path);
    for suffix in ["", "-wal", "-shm"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(e).with_context(|| format!("remove {}", Path::new(&file).display()));
            }
            _ => {}
        }
    }
    let started = Instant::now();
    generate(path, size, &ACCOUNTS, SystemClock.now())?;
    #[expect(clippy::print_stdout, reason = "a command-line tool reports on stdout")]
    {
        println!(
            "wrote {} in {:.1} s",
            path.display(),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}
