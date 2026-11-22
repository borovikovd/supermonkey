mod commands;
mod logging;
mod menu;
mod platform;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sm_core::activity::{Activity, Tasks};
use sm_core::clock::SystemClock;
use sm_core::{App, Config, Deps, GmailConfig};
use sm_gmail::{OAuthConfig, QuotaConfig, TokenFile, Units};
use sm_ipc::AppEvent;
use tauri::{AppHandle, Emitter as _, Manager as _, RunEvent, WindowEvent};
use tauri_plugin_window_state::StateFlags;
use tokio::sync::OnceCell;

pub use crate::logging::init_logging;
pub use crate::menu::{QUIT_MENU_ITEM, app_menu};
use crate::platform::TauriPlatform;

pub(crate) const MAIN_WINDOW: &str = "main";

#[cfg(all(feature = "fake", not(debug_assertions)))]
compile_error!("the `fake` feature is for debug builds only");

const SHOW_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

const PAGE_QUIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

const DEFAULT_QUOTA_PER_MIN: u32 = 5800;

pub(crate) struct Shell {
    pub(crate) core: Arc<App>,
    tasks: Tasks,
    quitting: AtomicBool,
    stopped: OnceCell<()>,
    pub(crate) connected: AtomicBool,
    #[cfg(feature = "fake")]
    _demo: Option<sm_testkit::Demo>,
}

impl Shell {
    async fn stop(&self) {
        self.stopped
            .get_or_init(|| async {
                self.tasks.stop().await;
                self.core.lifecycle().quit().await;
            })
            .await;
    }
}

struct Setup {
    config: Config,
    tokens: Arc<TokenFile>,
    #[cfg(feature = "fake")]
    demo: Option<sm_testkit::Demo>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    let built = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::all() - StateFlags::VISIBLE)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .menu(app_menu)
        .on_menu_event(|app, event| {
            if event.id() == QUIT_MENU_ITEM {
                request_quit(app);
            }
        })
        .setup(|app| {
            let shell = start_core(app.handle())?;
            app.manage(shell);
            show_if_never_connected(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            #[expect(
                clippy::wildcard_enum_match_arm,
                reason = "Tauri's enum is non-exhaustive, and the app acts on these two events only"
            )]
            match event {
                WindowEvent::CloseRequested { api, .. } if window.label() == MAIN_WINDOW => {
                    api.prevent_close();
                    if let Err(e) = window.hide() {
                        tracing::warn!(error = %e, "couldn't hide the main window");
                    }
                }
                WindowEvent::Focused(focused) => on_focus(window.app_handle(), *focused),
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::dispatch
        ])
        .build(tauri::generate_context!());
    #[expect(
        clippy::exit,
        reason = "the app was never built: there is no core to quit and no run loop to leave, and whoever launched it needs the failure status"
    )]
    let app = match built {
        Ok(app) => app,
        Err(e) => {
            tracing::error!(error = %e, "couldn't start Supermonkey");
            std::process::exit(1);
        }
    };
    app.run(|app, event| {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "Tauri's enum is non-exhaustive, and the app acts on these three events only"
        )]
        match event {
            RunEvent::Reopen { .. } => show_main(app),
            RunEvent::ExitRequested {
                code: None, api, ..
            } => {
                api.prevent_exit();
                request_quit(app);
            }
            RunEvent::Exit => finish_quit(app),
            _ => {}
        }
    });
}

fn start_core(app: &AppHandle) -> anyhow::Result<Shell> {
    let runtime = tauri::async_runtime::handle().inner().clone();
    let _in_runtime = runtime.enter();
    let setup = setup(app)?;
    let activity = Activity::new();
    let deps = Deps {
        platform: Arc::new(TauriPlatform(app.clone())),
        clock: Arc::new(SystemClock),
        tokens: setup.tokens,
        activity: Arc::clone(&activity),
    };
    let core = Arc::new(App::start(setup.config, deps)?);
    #[cfg(feature = "fake")]
    if let Some(demo) = &setup.demo {
        tauri::async_runtime::block_on(demo.register(&core))?;
    }
    core.lifecycle().launch();
    let tasks = Tasks::new(runtime.clone(), activity);
    start_web_bridge(&core, &tasks);
    Ok(Shell {
        core,
        tasks,
        quitting: AtomicBool::new(false),
        stopped: OnceCell::new(),
        connected: AtomicBool::new(false),
        #[cfg(feature = "fake")]
        _demo: setup.demo,
    })
}

fn setup(app: &AppHandle) -> anyhow::Result<Setup> {
    #[cfg(feature = "fake")]
    if cfg!(debug_assertions) && std::env::var_os("SUPERMONKEY_FAKE").is_some_and(|v| v == "1") {
        let demo = sm_testkit::Demo::start()?;
        return Ok(Setup {
            config: Config::in_data_dir(demo.data_dir(), true, demo.gmail_config()),
            tokens: demo.tokens(),
            demo: Some(demo),
        });
    }
    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    #[cfg_attr(
        not(debug_assertions),
        expect(unused_mut, reason = "only debug builds take a database override")
    )]
    let mut config = Config::in_data_dir(&data_dir, cfg!(debug_assertions), gmail_config());
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("SUPERMONKEY_DB") {
        config.db_path = std::path::PathBuf::from(path);
    }
    Ok(Setup {
        config,
        tokens: token_store(&data_dir),
        #[cfg(feature = "fake")]
        demo: None,
    })
}

fn gmail_config() -> GmailConfig {
    let per_minute = env!("SUPERMONKEY_QUOTA_PER_MIN")
        .parse()
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "SUPERMONKEY_QUOTA_PER_MIN is not a number; using the default");
            DEFAULT_QUOTA_PER_MIN
        });
    let oauth = OAuthConfig::google(env!("GOOGLE_CLIENT_ID"), env!("GOOGLE_CLIENT_SECRET"));
    GmailConfig::google(oauth, QuotaConfig::for_budget(Units(per_minute)))
}

fn token_store(data_dir: &Path) -> Arc<TokenFile> {
    Arc::new(TokenFile::new(data_dir.join(Config::TOKEN_FILE)))
}

fn wants_web_bridge() -> bool {
    std::env::var_os("SUPERMONKEY_WEB_BRIDGE").is_some_and(|v| v == "1")
}

#[cfg(all(debug_assertions, feature = "web"))]
fn start_web_bridge(core: &Arc<App>, tasks: &Tasks) {
    if !wants_web_bridge() {
        return;
    }
    let bridge = sm_bridge::WebBridge::new(Some(Arc::clone(core)), Arc::clone(tasks.activity()));
    tasks.spawn("web bridge", async move {
        if let Err(e) = sm_bridge::serve_local(bridge).await {
            tracing::warn!(error = %e, "the web bridge stopped");
        }
    });
}

#[cfg(not(all(debug_assertions, feature = "web")))]
fn start_web_bridge(_core: &Arc<App>, _tasks: &Tasks) {
    if wants_web_bridge() {
        tracing::warn!(
            "SUPERMONKEY_WEB_BRIDGE=1 needs a debug build with the `web` feature (--features web)"
        );
    }
}

fn show_if_never_connected(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("show deadline".into())
        .spawn(move || {
            std::thread::sleep(SHOW_DEADLINE);
            let connected = app
                .try_state::<Shell>()
                .is_some_and(|shell| shell.connected.load(Ordering::Acquire));
            if !connected {
                tracing::error!("the page did not connect to the core; showing the window anyway");
                show_main(&app);
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "couldn't start the window's show deadline");
    }
}

pub(crate) fn show_main(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    for (what, done) in [
        ("show", window.show()),
        ("unminimize", window.unminimize()),
        ("focus", window.set_focus()),
    ] {
        if let Err(e) = done {
            tracing::warn!(error = %e, step = what, "couldn't bring the main window forward");
        }
    }
}

fn on_focus(app: &AppHandle, focused: bool) {
    let Some(shell) = app.try_state::<Shell>() else {
        return;
    };
    let Some(clicked) = shell.core.lifecycle().focus(focused) else {
        return;
    };
    let event = AppEvent::NotificationOpen {
        account: clicked.account,
        thread: clicked.thread,
    };
    if let Err(e) = emit(app, &event) {
        tracing::warn!(error = %e, "couldn't open a clicked notification");
    }
}

fn emit(app: &AppHandle, event: &AppEvent) -> anyhow::Result<()> {
    let (name, payload) = event.parts()?;
    app.emit(&name, payload)?;
    Ok(())
}

fn request_quit(app: &AppHandle) {
    let Some(shell) = app.try_state::<Shell>() else {
        quit(app.clone());
        return;
    };
    if shell.quitting.load(Ordering::Acquire) {
        return;
    }
    if !shell.connected.load(Ordering::Acquire) {
        quit(app.clone());
    } else if let Err(e) = emit(app, &AppEvent::QuitRequested) {
        tracing::warn!(error = %e, "couldn't ask the page to quit; quitting");
        quit(app.clone());
    } else {
        quit_after_deadline(app);
    }
}

fn quit_after_deadline(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("quit deadline".into())
        .spawn(move || {
            std::thread::sleep(PAGE_QUIT_DEADLINE);
            let quitting = app
                .try_state::<Shell>()
                .is_some_and(|shell| shell.quitting.load(Ordering::Acquire));
            if !quitting {
                tracing::warn!("the page did not answer the quit in time; quitting");
                quit(app);
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "couldn't start the quit deadline");
    }
}

pub(crate) fn quit(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Some(shell) = app.try_state::<Shell>() {
            if shell.quitting.swap(true, Ordering::AcqRel) {
                return;
            }
            shell.stop().await;
        }
        app.exit(0);
    });
}

fn finish_quit(app: &AppHandle) {
    if let Some(shell) = app.try_state::<Shell>() {
        shell.quitting.store(true, Ordering::Release);
        tauri::async_runtime::block_on(shell.stop());
    }
}
