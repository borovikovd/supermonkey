//! Logs (ARCHITECTURE.md, section 9): `tracing` to stderr, one line per
//! event. Our crates log at debug in debug builds; everything else at info,
//! and release builds keep warnings and errors only. Noisy third-party
//! targets are filtered by name rather than by raising the global level.

use std::io::IsTerminal as _;

use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

/// The app's own crates, by target prefix.
const OURS: [&str; 10] = [
    "ubermensch",
    "e2e_server",
    "ub_bridge",
    "ub_core",
    "ub_domain",
    "ub_fake_gmail",
    "ub_gmail",
    "ub_mime",
    "ub_store",
    "ub_testkit",
];

/// Installs the stderr logger, once per process. `log` records (Tauri's,
/// the plugins') go through it too.
pub fn init_logging() {
    let installed = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(std::io::stderr().is_terminal()),
        )
        .with(filter())
        .try_init();
    if let Err(e) = installed {
        // Another logger is installed already and gets this line.
        tracing::warn!(error = %e, "a logger was already installed");
    }
}

/// Which targets log at which level.
fn filter() -> Targets {
    let (ours, others) = if cfg!(debug_assertions) {
        (LevelFilter::DEBUG, LevelFilter::INFO)
    } else {
        (LevelFilter::WARN, LevelFilter::WARN)
    };
    Targets::new()
        .with_default(others)
        .with_targets(OURS.map(|target| (target, ours)))
        // The sanitizer's HTML parser warns about every malformed table in
        // mail ("foster parenting not implemented"); ammonia still cleans it.
        .with_target("html5ever", LevelFilter::ERROR)
}
