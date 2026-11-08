//! The Tauri shell (ARCHITECTURE.md, section 3): plugins, the window and
//! Dock, the [`Platform`](sm_core::Platform) port, and commands that call
//! `sm_bridge`. No logic beyond wiring: the core decides.
//!
//! - Closing the window hides it; the app keeps syncing in the Dock, and a
//!   Dock click shows it again.
//! - Cmd+Q, the app menu's own Quit, asks a connected page to close an open
//!   composer first, then the page calls `quit_now`; quitting waits for the
//!   user's last actions to commit (SPEC UI state, Actions), then exits. A
//!   page that has not asked within 10 s is quit without.
//! - When macOS terminates the app (Quit in the Dock, a logout, a
//!   shutdown), the page can't answer: the exit still waits, within the
//!   core's own deadline, for the UI actions the core has queued.
//! - Debug builds with the `web` feature serve the web bridge on
//!   127.0.0.1:1430 when `SUPERMONKEY_WEB_BRIDGE=1`, for the UI in a browser.
//! - Debug builds with the `fake` feature run demo mode when
//!   `SUPERMONKEY_FAKE=1` (`vp run demo`): an in-process fake Gmail with demo
//!   mail and two accounts, on fresh data in its own folder each launch.

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
use sm_gmail::{OAuthConfig, QuotaConfig, TokenFile, TokenStore, Units};
use sm_ipc::AppEvent;
use tauri::{AppHandle, Emitter as _, Manager as _, RunEvent, WindowEvent};
use tauri_plugin_window_state::StateFlags;
use tokio::sync::OnceCell;

pub use crate::logging::init_logging;
pub use crate::menu::{QUIT_MENU_ITEM, app_menu};
use crate::platform::TauriPlatform;

/// The main window's label (`tauri.conf.json`).
pub(crate) const MAIN_WINDOW: &str = "main";

// Demo mode and the E2E server link the fake Gmail and the test control
// server: never into a release build.
#[cfg(all(feature = "fake", not(debug_assertions)))]
compile_error!("the `fake` feature is for debug builds only");

/// Endpoint overrides a debug build takes (`gmail_config`).
const ENDPOINT_OVERRIDES: [&str; 4] = [
    "SUPERMONKEY_GMAIL_BASE",
    "SUPERMONKEY_OAUTH_AUTH_URL",
    "SUPERMONKEY_OAUTH_TOKEN_URL",
    "SUPERMONKEY_OAUTH_REVOKE_URL",
];

/// How long after launch the window shows even if the page has not
/// connected (SPEC Window).
const SHOW_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// How long the page has to answer a quit before the app quits without it
/// (SPEC Actions): longer than the page's own 5 s wait for the core.
const PAGE_QUIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// Quota units a minute per account when the build sets no usable
/// `SUPERMONKEY_QUOTA_PER_MIN` (SPEC Sync).
const DEFAULT_QUOTA_PER_MIN: u32 = 5800;

/// What the commands and window events share.
pub(crate) struct Shell {
    pub(crate) core: Arc<App>,
    /// The shell's own tasks (the debug web bridge), stopped on quit.
    tasks: Tasks,
    /// Set by the first quit, so a second Cmd+Q doesn't quit twice.
    quitting: AtomicBool,
    /// Set once the shell's tasks and the core have stopped ([`Shell::stop`]).
    stopped: OnceCell<()>,
    /// Set once the page has connected and the window has been shown.
    pub(crate) connected: AtomicBool,
    /// Demo mode's fake Gmail, kept running for as long as the app.
    #[cfg(feature = "fake")]
    _demo: Option<sm_testkit::Demo>,
}

impl Shell {
    /// Stops the shell's tasks and quits the core, which commits the UI
    /// actions it has queued within its own deadline. Runs once: a later
    /// caller waits for the first to finish.
    async fn stop(&self) {
        self.stopped
            .get_or_init(|| async {
                self.tasks.stop().await;
                self.core.lifecycle().quit().await;
            })
            .await;
    }
}

/// Where the core keeps its data, which Gmail it talks to, and where its
/// refresh tokens live.
struct Setup {
    config: Config,
    tokens: Arc<dyn TokenStore>,
    #[cfg(feature = "fake")]
    demo: Option<sm_testkit::Demo>,
}

/// Starts the app.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    let built = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        // Size and position only: the shell decides when the window shows.
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
        .on_window_event(|window, event| match event {
            // Closing the window hides it; the app keeps syncing in the Dock.
            WindowEvent::CloseRequested { api, .. } if window.label() == MAIN_WINDOW => {
                api.prevent_close();
                if let Err(e) = window.hide() {
                    tracing::warn!(error = %e, "couldn't hide the main window");
                }
            }
            WindowEvent::Focused(focused) => on_focus(window.app_handle(), *focused),
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::connect,
            commands::dispatch
        ])
        .build(tauri::generate_context!());
    let app = match built {
        Ok(app) => app,
        Err(e) => {
            tracing::error!(error = %e, "couldn't start Supermonkey");
            std::process::exit(1);
        }
    };
    app.run(|app, event| match event {
        // Clicking the Dock icon shows the window again.
        RunEvent::Reopen { .. } => show_main(app),
        // Tauri asks this when the last window is gone, which hiding on
        // close never lets happen; quit as Cmd+Q does all the same.
        RunEvent::ExitRequested {
            code: None, api, ..
        } => {
            api.prevent_exit();
            request_quit(app);
        }
        RunEvent::Exit => finish_quit(app),
        _ => {}
    });
}

/// Opens the core and launches it: in the app's data folder, or on demo
/// data in demo mode.
fn start_core(app: &AppHandle) -> anyhow::Result<Shell> {
    let runtime = tauri::async_runtime::handle().inner().clone();
    // The core spawns its tasks on the runtime it starts in, and so does
    // demo mode's fake Gmail.
    let _in_runtime = runtime.enter();
    let setup = setup(app)?;
    let activity = Activity::new();
    let deps = Deps {
        platform: Arc::new(TauriPlatform(app.clone())),
        clock: Arc::new(SystemClock),
        tokens: setup.tokens,
        activity: Arc::clone(&activity),
    };
    let core = Arc::new(App::start(&setup.config, deps)?);
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

/// The app's data folder, Google's Gmail, and the token file; or, in demo
/// mode, the demo's.
fn setup(app: &AppHandle) -> anyhow::Result<Setup> {
    #[cfg(feature = "fake")]
    if cfg!(debug_assertions) && std::env::var("SUPERMONKEY_FAKE").as_deref() == Ok("1") {
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
    // Debug only: a perf fixture or other test database (`perf-fixture`).
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

/// Google's Gmail with the OAuth client and budget the build baked in
/// (`build.rs`). Debug builds may point every endpoint elsewhere, such as at
/// a fake.
fn gmail_config() -> GmailConfig {
    let per_minute = env!("SUPERMONKEY_QUOTA_PER_MIN")
        .parse()
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "SUPERMONKEY_QUOTA_PER_MIN is not a number; using the default");
            DEFAULT_QUOTA_PER_MIN
        });
    let oauth = OAuthConfig::google(env!("GOOGLE_CLIENT_ID"), env!("GOOGLE_CLIENT_SECRET"));
    #[cfg_attr(
        not(debug_assertions),
        expect(unused_mut, reason = "only debug builds take endpoint overrides")
    )]
    let mut gmail = GmailConfig::google(oauth, QuotaConfig::for_budget(Units(per_minute)));
    #[cfg(debug_assertions)]
    {
        let set = |name: &str, field: &mut String| {
            if let Ok(value) = std::env::var(name) {
                *field = value;
            }
        };
        let [base, auth, token, revoke] = ENDPOINT_OVERRIDES;
        set(base, &mut gmail.base_url);
        set(auth, &mut gmail.oauth.auth_url);
        set(token, &mut gmail.oauth.token_url);
        set(revoke, &mut gmail.oauth.revoke_url);
    }
    gmail
}

/// The refresh-token file in the data folder. A debug build with any
/// endpoint pointed elsewhere keeps its tokens in memory, so the user's
/// refresh tokens are never sent to another token or revoke endpoint.
fn token_store(data_dir: &Path) -> Arc<dyn TokenStore> {
    if cfg!(debug_assertions)
        && ENDPOINT_OVERRIDES
            .iter()
            .any(|name| std::env::var_os(name).is_some())
    {
        return Arc::new(sm_gmail::MemoryTokenStore::default());
    }
    Arc::new(TokenFile::new(data_dir.join(Config::TOKEN_FILE)))
}

/// Whether `SUPERMONKEY_WEB_BRIDGE=1` asks for the debug web bridge.
fn wants_web_bridge() -> bool {
    std::env::var("SUPERMONKEY_WEB_BRIDGE").as_deref() == Ok("1")
}

/// Serves the debug web bridge when asked for, in debug builds with the
/// `web` feature.
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

/// Builds without the bridge only say why it isn't there.
#[cfg(not(all(debug_assertions, feature = "web")))]
fn start_web_bridge(_core: &Arc<App>, _tasks: &Tasks) {
    if wants_web_bridge() {
        tracing::warn!(
            "SUPERMONKEY_WEB_BRIDGE=1 needs a debug build with the `web` feature (--features web)"
        );
    }
}

/// Shows the window [`SHOW_DEADLINE`] after launch if the page has not
/// connected by then, which shows it: a page that failed to load still gets
/// a window, so the app is never running with nothing to quit from.
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

/// Shows, unhides, and focuses the main window.
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

/// The window gained or lost focus. A focus that counts as a click on the
/// last notification shows its thread.
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

/// Sends the page an app event.
fn emit(app: &AppHandle, event: &AppEvent) -> anyhow::Result<()> {
    let (name, payload) = event.parts()?;
    app.emit(&name, payload)?;
    Ok(())
}

/// The app menu's Quit (`Cmd+Q`): a page that has connected closes an open
/// composer and waits for its last actions to commit, then calls
/// `quit_now`, or [`PAGE_QUIT_DEADLINE`] later the app quits anyway. The
/// window may be hidden or minimized: its page can still hold a composer. A
/// page that never connected can't answer, so the app quits at once.
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

/// Quits after [`PAGE_QUIT_DEADLINE`] if the page has not asked to by then:
/// a hung page never keeps the app open. A quit the page asked for first
/// makes this one do nothing.
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

/// Exits once the core has committed the UI actions queued so far, so the
/// last ones the page sent are not lost; queued sync work is dropped.
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

/// The process is about to end: after [`quit`], or because macOS terminated
/// the app (Quit in the Dock, a logout, a shutdown, a script), which goes
/// straight to `RunEvent::Exit` with no chance for the page to answer. Waits
/// on the main thread until the core has committed the UI actions it has
/// queued, within its own deadline; after [`quit`] that is already done.
fn finish_quit(app: &AppHandle) {
    if let Some(shell) = app.try_state::<Shell>() {
        shell.quitting.store(true, Ordering::Release);
        tauri::async_runtime::block_on(shell.stop());
    }
}
