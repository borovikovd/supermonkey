//! The E2E server (`sm_testkit::run`): the real core against the fake Gmail,
//! behind the web bridge and the `/__test` control API.
//!
//! `e2e-server [--port N] [--dist PATH] [--now MS]`: port 0 picks a free
//! one; the UI comes from `dist/` (build it with `vp build`); app time starts
//! at `MS` (Unix milliseconds, default the system clock). Set `TZ` for the
//! local time zone.

use std::path::PathBuf;

use anyhow::Context as _;
use sm_core::clock::{Clock as _, SystemClock};
use sm_domain::Millis;

/// The value after `--name` on the command line.
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
    supermonkey_lib::init_logging();
    let port = arg("--port").map_or(Ok(0), |p| p.parse().context("--port"))?;
    let dist = arg("--dist").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist"),
        PathBuf::from,
    );
    let now = match arg("--now") {
        Some(ms) => Millis(ms.parse().context("--now")?),
        None => SystemClock.now(),
    };
    sm_testkit::run(port, dist, now).await
}
