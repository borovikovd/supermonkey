//! Ubermensch: a keyboard-first Gmail client for macOS.

// SQL rows are read into plain tuples throughout.
#![allow(clippy::type_complexity)]

pub mod accounts;
pub mod actions;
pub mod commands;
pub mod db;
pub mod drafts;
pub mod engine;
pub mod fixture;
pub mod labels;
pub mod mail;
pub mod model;
pub mod notify;
pub mod outbox;
pub mod queue;
pub mod splits;
pub mod subs;
pub mod threads;
pub mod undo;
pub mod writer;

use std::path::PathBuf;

use tauri::{Emitter, Manager, RunEvent, WindowEvent};

use crate::writer::Writer;

/// Shared state for commands.
pub struct Core {
    pub writer: Writer,
    pub reads: db::ReadPool,
    pub data_dir: PathBuf,
}

fn db_path(data_dir: &std::path::Path) -> PathBuf {
    std::env::var("UBERMENSCH_DB").map_or_else(|_| data_dir.join("ubermensch.db"), PathBuf::from)
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                let _ = env_logger_init();
            }
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let _ = drafts::DRAFT_FILES.set(data_dir.join("drafts"));
            let path = db_path(&data_dir);
            let (effects_tx, effects_rx) = tokio::sync::mpsc::unbounded_channel();
            let writer = Writer::start(path.clone(), effects_tx)?;
            let reads = db::ReadPool::new(&path, 4);
            app.manage(Core {
                writer: writer.clone(),
                reads: reads.clone(),
                data_dir: data_dir.clone(),
            });
            engine::start(app.handle().clone(), writer, reads, data_dir, effects_rx);
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Closing the window hides it; the app keeps syncing in the Dock.
            WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                api.prevent_close();
                let _ = window.hide();
            }
            WindowEvent::Focused(focused) => {
                let Some(engine) = window.app_handle().try_state::<engine::EngineRef>() else {
                    return;
                };
                if *focused {
                    if let Some(n) = engine.inner().on_focus() {
                        let _ = window.app_handle().emit(
                            "notification-open",
                            serde_json::json!({ "account": n.account, "thread": n.thread }),
                        );
                    }
                } else {
                    engine.on_blur();
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::submit,
            commands::fetch_rows,
            commands::get_bodies,
            commands::autocomplete,
            commands::add_account,
            commands::sign_in_again,
            commands::open_attachment,
            commands::quit_now,
            commands::perf_report,
            commands::snooze_options,
            commands::add_attachment,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Ubermensch");

    app.run(|app, event| match event {
        // Clicking the Dock icon shows the window again.
        RunEvent::Reopen { .. } => show_main(app),
        // Cmd+Q: let the UI close an open composer first, then it calls quit_now.
        RunEvent::ExitRequested {
            code: None, api, ..
        } => {
            let visible = app
                .get_webview_window("main")
                .and_then(|w| w.is_visible().ok())
                .unwrap_or(false);
            if visible {
                api.prevent_exit();
                let _ = app.emit("quit-requested", ());
            }
        }
        _ => {}
    });
}

fn env_logger_init() -> Result<(), log::SetLoggerError> {
    struct Stderr;
    impl log::Log for Stderr {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            m.level() <= log::Level::Info || m.target().starts_with("ubermensch")
        }
        fn log(&self, r: &log::Record<'_>) {
            if self.enabled(r.metadata()) {
                eprintln!("[{}] {}: {}", r.level(), r.target(), r.args());
            }
        }
        fn flush(&self) {}
    }
    log::set_logger(&Stderr).map(|()| log::set_max_level(log::LevelFilter::Debug))
}
