mod commands;
pub mod logging;
pub mod menu;
mod platform;

use tauri::{Manager as _, RunEvent, WindowEvent};
use tauri_plugin_window_state::StateFlags;

use crate::core::{Clock, Core, CoreConfig, log_err};
use crate::gmail::{GMAIL_BASE_URL, OAuthConfig, QuotaConfig, Units};
use crate::session::Session;
use platform::{TauriHost, TauriPlatform, show_main_window};

#[cfg(feature = "web")]
const DEV_BRIDGE_ADDR: &str = "127.0.0.1:1430";

#[cfg(any(feature = "web", feature = "fake"))]
fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| {
        let s = v.to_string_lossy();
        !s.is_empty() && s != "0" && !s.eq_ignore_ascii_case("false")
    })
}

fn production_config(data_dir: std::path::PathBuf) -> CoreConfig {
    let budget: u32 = env!("SUPERMONKEY_QUOTA_PER_MIN").parse().unwrap_or(5800);
    CoreConfig {
        data_dir,
        gmail_base_url: GMAIL_BASE_URL.to_owned(),
        oauth: OAuthConfig::google(env!("GOOGLE_CLIENT_ID"), env!("GOOGLE_CLIENT_SECRET")),
        quota: QuotaConfig::for_budget(Units(budget)),
        clock: Clock::system(),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "wires Tauri plugins, menu, core startup, optional dev bridge, and lifecycle events"
)]
pub fn run() {
    logging::init();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(StateFlags::all() - StateFlags::VISIBLE)
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::dispatch,
        ])
        .setup(|app| {
            let app_menu = menu::build(app.handle())?;
            app.set_menu(app_menu)?;

            #[cfg(feature = "web")]
            let web_events =
                env_flag("SUPERMONKEY_WEB_BRIDGE").then(crate::session::web::WebEvents::new);

            let config = {
                #[cfg(feature = "fake")]
                {
                    if env_flag("SUPERMONKEY_FAKE") {
                        let demo = tauri::async_runtime::block_on(async {
                            crate::testkit::demo::Demo::start()
                        })?;
                        let cfg = demo.core_config();
                        app.manage(demo);
                        cfg
                    } else {
                        let data_dir = app.path().app_data_dir()?;
                        std::fs::create_dir_all(&data_dir)?;
                        production_config(data_dir)
                    }
                }
                #[cfg(not(feature = "fake"))]
                {
                    let data_dir = app.path().app_data_dir()?;
                    std::fs::create_dir_all(&data_dir)?;
                    production_config(data_dir)
                }
            };

            let platform = Box::new(TauriPlatform::new(app.handle().clone()));
            let host = Box::new(TauriHost::new(
                app.handle().clone(),
                #[cfg(feature = "web")]
                web_events.clone(),
            ));
            let session = tauri::async_runtime::block_on(async {
                let core = Core::open(config, platform)
                    .map_err(|err| std::io::Error::other(err.to_string()))?;
                core.start()
                    .await
                    .map_err(|err| std::io::Error::other(err.to_string()))?;
                Ok::<_, std::io::Error>(Session::new(core, host))
            })?;

            #[cfg(feature = "web")]
            if let Some(events) = web_events {
                spawn_dev_bridge(session.clone(), events);
            }

            app.manage(session);
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id() == menu::QUIT_ID {
                app.state::<Session>().request_quit();
            }
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                log_err("hide window on close", window.hide());
            } else if let WindowEvent::Focused(focused) = event {
                let _open = window.app_handle().state::<Session>().focus(*focused);
            }
        });

    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(err) => {
            tracing::error!(error = %err, "failed to build tauri application");
            return;
        }
    };

    app.run(|app_handle, event| {
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } = event
        {
            show_main_window(app_handle);
        }
        if let RunEvent::ExitRequested { code: None, .. } = event {
            tauri::async_runtime::block_on(app_handle.state::<Session>().terminate());
        }
    });
}

#[cfg(feature = "web")]
fn spawn_dev_bridge(session: Session, events: crate::session::web::WebEvents) {
    let state = crate::session::web::BridgeState::new(
        None,
        events,
        vec![
            "http://localhost:5173".to_owned(),
            "http://127.0.0.1:5173".to_owned(),
            format!("http://{DEV_BRIDGE_ADDR}"),
            "http://localhost:1430".to_owned(),
        ],
    );
    state.set_session(Some(session));
    tauri::async_runtime::spawn(async move {
        match tokio::net::TcpListener::bind(DEV_BRIDGE_ADDR).await {
            Ok(listener) => {
                tracing::info!(addr = DEV_BRIDGE_ADDR, "dev web bridge listening");
                log_err(
                    "serve dev web bridge",
                    axum::serve(listener, crate::session::web::router(state)).await,
                );
            }
            Err(err) => {
                tracing::warn!(addr = DEV_BRIDGE_ADDR, error = %err, "failed to bind dev web bridge");
            }
        }
    });
}
