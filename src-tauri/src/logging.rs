use std::io::IsTerminal as _;

use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

const OURS: [&str; 10] = [
    "supermonkey",
    "e2e_server",
    "sm_bridge",
    "sm_core",
    "sm_domain",
    "sm_fake_gmail",
    "sm_gmail",
    "sm_mime",
    "sm_store",
    "sm_testkit",
];

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
        tracing::warn!(error = %e, "a logger was already installed");
    }
}

fn filter() -> Targets {
    let (ours, others) = if cfg!(debug_assertions) {
        (LevelFilter::DEBUG, LevelFilter::INFO)
    } else {
        (LevelFilter::WARN, LevelFilter::WARN)
    };
    Targets::new()
        .with_default(others)
        .with_targets(OURS.map(|target| (target, ours)))
        .with_target("html5ever", LevelFilter::ERROR)
}
