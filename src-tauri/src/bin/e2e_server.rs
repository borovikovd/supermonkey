use std::path::PathBuf;

use anyhow::Context as _;
use sm_core::clock::{Clock as _, SystemClock};
use sm_domain::Millis;

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
