use std::path::Path;

use tauri::{AppHandle, Emitter as _, Manager as _, Runtime};
use tauri_plugin_notification::NotificationExt as _;
use tauri_plugin_opener::OpenerExt as _;

use crate::core::{CoreError, Notification, Platform, log_err};
use crate::ipc::AppEvent;
use crate::session::Host;
#[cfg(feature = "web")]
use crate::session::web::WebEvents;

pub(super) struct TauriPlatform<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> std::fmt::Debug for TauriPlatform<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TauriPlatform").finish_non_exhaustive()
    }
}

impl<R: Runtime> TauriPlatform<R> {
    #[must_use]
    pub(super) const fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> Platform for TauriPlatform<R> {
    fn open_url(&self, url: &str) -> Result<(), CoreError> {
        self.app
            .opener()
            .open_url(url, None::<&str>)
            .map_err(|err| CoreError::other(err.to_string()))
    }

    fn open_file(&self, path: &Path) -> Result<(), CoreError> {
        self.app
            .opener()
            .open_path(path.to_string_lossy().as_ref(), None::<&str>)
            .map_err(|err| CoreError::other(err.to_string()))
    }

    fn notify(&self, notification: Notification) {
        log_err(
            "show desktop notification",
            self.app
                .notification()
                .builder()
                .title(notification.title)
                .body(notification.body)
                .show(),
        );
    }

    fn set_badge(&self, count: u32) {
        if let Some(window) = self.app.get_webview_window("main") {
            let value = (count > 0).then(|| i64::from(count));
            log_err("update window badge count", window.set_badge_count(value));
        }
    }
}

pub(super) struct TauriHost<R: Runtime> {
    app: AppHandle<R>,
    #[cfg(feature = "web")]
    web_events: Option<WebEvents>,
}

impl<R: Runtime> std::fmt::Debug for TauriHost<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TauriHost").finish_non_exhaustive()
    }
}

impl<R: Runtime> TauriHost<R> {
    #[must_use]
    pub(super) const fn new(
        app: AppHandle<R>,
        #[cfg(feature = "web")] web_events: Option<WebEvents>,
    ) -> Self {
        Self {
            app,
            #[cfg(feature = "web")]
            web_events,
        }
    }
}

pub(super) fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        log_err("unminimize main window", window.unminimize());
        log_err("show main window", window.show());
        log_err("focus main window", window.set_focus());
    }
}

impl<R: Runtime> Host for TauriHost<R> {
    fn show_window(&self) {
        show_main_window(&self.app);
    }

    fn emit(&self, event: AppEvent) {
        #[cfg(feature = "web")]
        if let Some(web) = &self.web_events {
            web.emit(event);
        }
        if let Ok((name, payload)) = event.parts() {
            log_err("emit tauri event", self.app.emit(&name, payload));
        }
    }

    fn exit(&self) {
        self.app.exit(0);
    }
}
