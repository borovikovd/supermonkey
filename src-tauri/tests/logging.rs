//! The stderr logger keeps the sanitizer's parser warnings out of the log.

use log::{Level, log_enabled};

#[test]
fn html_parser_warnings_are_dropped_and_ours_are_kept() {
    assert!(ubermensch_lib::stderr_logger_init().is_ok());
    assert!(!log_enabled!(target: "html5ever::tree_builder", Level::Warn));
    assert!(log_enabled!(target: "html5ever::tree_builder", Level::Error));
    assert!(log_enabled!(target: "ubermensch_lib::engine::sync", Level::Warn));
    assert!(log_enabled!(target: "reqwest::connect", Level::Warn));
}
