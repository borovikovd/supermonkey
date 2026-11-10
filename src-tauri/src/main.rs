//! The Supermonkey app's binary: it runs the Tauri shell, which lives in the
//! library crate.

// No console window on Windows in release; ignored on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    supermonkey_lib::run();
}
