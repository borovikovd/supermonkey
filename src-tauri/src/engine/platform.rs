//! What the engine asks of the operating system: open a URL or file, post a
//! notification, set the Dock badge, bring the window forward. The app uses
//! Tauri; tests and the E2E server record the calls instead.

use std::path::Path;

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

use crate::LogErr as _;

pub trait Platform: Send + Sync + 'static {
    /// Opens a URL in the default browser (the Google sign-in page).
    fn open_url(&self, url: &str);
    /// Opens a file with its default app (an attachment).
    fn open_path(&self, path: &Path) -> anyhow::Result<()>;
    fn notify(&self, title: &str, body: &str);
    /// The Dock badge; `None` clears it.
    fn set_badge(&self, count: Option<i64>);
    /// Brings the main window forward (after a sign-in completes).
    fn focus_window(&self);
}

/// Does nothing: for tests that run the engine without a window.
pub struct NoPlatform;

impl Platform for NoPlatform {
    fn open_url(&self, _url: &str) {}
    fn open_path(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }
    fn notify(&self, _title: &str, _body: &str) {}
    fn set_badge(&self, _count: Option<i64>) {}
    fn focus_window(&self) {}
}

pub struct TauriPlatform(pub AppHandle);

impl Platform for TauriPlatform {
    fn open_url(&self, url: &str) {
        self.0
            .opener()
            .open_url(url, None::<&str>)
            .log_err("open URL");
    }

    fn open_path(&self, path: &Path) -> anyhow::Result<()> {
        self.0
            .opener()
            .open_path(path.to_string_lossy(), None::<&str>)?;
        Ok(())
    }

    fn notify(&self, title: &str, body: &str) {
        self.0
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .log_err("show notification");
    }

    fn set_badge(&self, count: Option<i64>) {
        if let Some(w) = self.0.get_webview_window("main") {
            w.set_badge_count(count).log_err("set badge count");
        }
    }

    fn focus_window(&self) {
        if let Some(w) = self.0.get_webview_window("main") {
            w.set_focus().log_err("focus the main window");
        }
    }
}
