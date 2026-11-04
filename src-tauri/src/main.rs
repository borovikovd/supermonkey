// No console window on Windows in release; ignored on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ubermensch_lib::run();
}
