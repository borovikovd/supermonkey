use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

fn build_filter() -> Targets {
    let default_level = if cfg!(debug_assertions) {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    Targets::new()
        .with_default(default_level)
        .with_target("html5ever", LevelFilter::WARN)
        .with_target("markup5ever", LevelFilter::WARN)
        .with_target("ammonia", LevelFilter::WARN)
        .with_target("selectors", LevelFilter::WARN)
        .with_target("cssparser", LevelFilter::WARN)
        .with_target("reqwest", LevelFilter::WARN)
        .with_target("hyper", LevelFilter::WARN)
        .with_target("hyper_util", LevelFilter::WARN)
        .with_target("h2", LevelFilter::WARN)
        .with_target("rustls", LevelFilter::WARN)
        .with_target("tao", LevelFilter::WARN)
        .with_target("wry", LevelFilter::WARN)
        .with_target("tauri_runtime_wry", LevelFilter::WARN)
        .with_target("tower_http", LevelFilter::WARN)
}

pub fn init() {
    let _initialized = tracing_subscriber::registry()
        .with(build_filter())
        .with(tracing_subscriber::fmt::layer().with_target(true))
        .try_init()
        .is_ok();
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use super::*;

    #[test]
    fn noisy_html_and_transport_targets_are_filtered_to_warn() {
        let filter = build_filter();
        for target in [
            "html5ever::tree_builder",
            "html5ever::tokenizer",
            "markup5ever",
            "reqwest::connect",
            "hyper::proto",
            "tauri_runtime_wry",
        ] {
            assert!(
                !filter.would_enable(target, &Level::DEBUG),
                "{target} debug logs must be suppressed"
            );
            assert!(
                !filter.would_enable(target, &Level::INFO),
                "{target} info logs must be suppressed"
            );
            assert!(
                filter.would_enable(target, &Level::WARN),
                "{target} warnings stay enabled"
            );
        }
        assert!(
            filter.would_enable("supermonkey_lib::shell", &Level::INFO),
            "app info logs stay enabled"
        );
    }
}
