use std::path::PathBuf;

use supermonkey_lib::testkit::fixture::generate_perf_db;

#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "perf-fixture CLI reports output path and stats on stdout and errors on stderr"
)]
fn main() {
    let mut out = PathBuf::from("/tmp/supermonkey-perf.db");
    let mut threads = 5_000usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                if let Some(v) = args.next() {
                    out = PathBuf::from(v);
                }
            }
            "--threads" => {
                if let Some(v) = args.next()
                    && let Ok(n) = v.parse()
                {
                    threads = n;
                }
            }
            other if !other.starts_with("--") => {
                out = PathBuf::from(other);
            }
            _ => {}
        }
    }

    match generate_perf_db(&out, threads) {
        Ok(stats) => {
            println!(
                "wrote {} ({} threads, {} messages, {} contacts)",
                out.display(),
                stats.threads,
                stats.messages,
                stats.contacts
            );
        }
        Err(err) => {
            eprintln!("failed to generate perf fixture: {err}");
        }
    }
}
