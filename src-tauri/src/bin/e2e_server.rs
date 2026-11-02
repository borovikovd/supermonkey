//! The E2E test server (see `engine::testserver`).
//!
//! `e2e-server [--port N] [--dist PATH]`: port 0 picks a free one; the UI
//! comes from `dist/` (build it with `vp build`).

use std::path::PathBuf;

fn arg(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == name {
            return args.next();
        }
    }
    None
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Only fails when a logger is already set.
    let _ = ubermensch_lib::stderr_logger_init();
    let port = arg("--port").map_or(Ok(0), |p| p.parse())?;
    let dist = arg("--dist").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist"),
        PathBuf::from,
    );
    ubermensch_lib::engine::testserver::run(port, dist).await
}
