use std::path::Path;
use std::time::Instant;

use anyhow::Context as _;
use sm_core::clock::{Clock as _, SystemClock};
use sm_testkit::fixture::{PERF, generate};

const ACCOUNTS: [&str; 2] = ["alex@example.com", "sam@example.com"];

fn main() -> anyhow::Result<()> {
    supermonkey_lib::init_logging();
    let path = std::env::args()
        .nth(1)
        .context("usage: perf-fixture <db path>")?;
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
    generate(path, PERF, &ACCOUNTS, SystemClock.now())?;
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
