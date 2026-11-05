//! The Tauri [`Platform`] (ARCHITECTURE.md, section 8.3): links and files
//! through the opener plugin, notifications, the Dock badge, and the main
//! window's focus.

use std::path::Path;

use tauri::{AppHandle, Manager as _};
use tauri_plugin_notification::NotificationExt as _;
use tauri_plugin_opener::OpenerExt as _;
use ub_core::{Notification, Platform, PlatformError};

/// The operating system, through Tauri and its plugins.
pub(crate) struct TauriPlatform(pub(crate) AppHandle);

impl TauriPlatform {
    fn main_window(&self) -> Option<tauri::WebviewWindow> {
        self.0.get_webview_window(crate::MAIN_WINDOW)
    }
}

impl Platform for TauriPlatform {
    fn open_url(&self, url: &str) -> Result<(), PlatformError> {
        self.0
            .opener()
            .open_url(url, None::<&str>)
            .map_err(|e| PlatformError(format!("couldn't open the link: {e}")))
    }

    fn open_sign_in(&self, url: &str) -> Result<(), PlatformError> {
        self.0
            .opener()
            .open_url(url, None::<&str>)
            .map_err(|e| PlatformError(format!("couldn't open the sign-in page: {e}")))
    }

    fn open_path(&self, path: &Path) -> Result<(), PlatformError> {
        self.0
            .opener()
            .open_path(path.to_string_lossy(), None::<&str>)
            .map_err(|e| PlatformError(format!("couldn't open the file: {e}")))
    }

    fn notify(&self, notification: &Notification) {
        let shown = self
            .0
            .notification()
            .builder()
            .title(&notification.title)
            .body(notification.text())
            .show();
        if let Err(e) = shown {
            tracing::warn!(error = %e, "couldn't post a notification");
        }
    }

    fn set_badge(&self, count: Option<u32>) {
        let Some(window) = self.main_window() else {
            return;
        };
        if let Err(e) = window.set_badge_count(count.map(i64::from)) {
            tracing::warn!(error = %e, "couldn't set the Dock badge");
        }
    }

    fn focus_window(&self) {
        let Some(window) = self.main_window() else {
            return;
        };
        if let Err(e) = window.set_focus() {
            tracing::warn!(error = %e, "couldn't focus the main window");
        }
    }
}
