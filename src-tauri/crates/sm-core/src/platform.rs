//! What the core asks of the operating system (ARCHITECTURE.md, section
//! 8.3): open a link or a file, open the sign-in page, post a notification,
//! set the Dock badge, bring the window forward. The app implements it with
//! Tauri; the E2E server records the calls instead.

use std::path::Path;

use sm_domain::{AccountId, ThreadId};

/// The operating system, as the core uses it. Calls come from async tasks,
/// never from the writer thread, and never under a lock.
pub trait Platform: Send + Sync {
    /// Opens a link in the default browser or mail app: a link in a
    /// message, a list's unsubscribe page.
    fn open_url(&self, url: &str) -> Result<(), PlatformError>;

    /// Opens Google's consent page in the system browser for a sign-in.
    fn open_sign_in(&self, url: &str) -> Result<(), PlatformError>;

    /// Opens a file with its default app: a downloaded attachment.
    fn open_path(&self, path: &Path) -> Result<(), PlatformError>;

    /// Posts a macOS notification.
    fn notify(&self, notification: &Notification);

    /// Sets the Dock badge; `None` clears it.
    fn set_badge(&self, count: Option<u32>);

    /// Brings the main window forward, as after a sign-in completes.
    fn focus_window(&self);
}

/// Why the operating system refused a request.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PlatformError(pub String);

/// A macOS notification for new mail (SPEC Notifications).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Notification {
    /// The account the mail came to.
    pub account: AccountId,
    /// That account's address, which the notification names.
    pub account_email: String,
    /// The thread to open on click; `None` for a summary of several messages.
    pub thread: Option<ThreadId>,
    /// The title line.
    pub title: String,
    /// The body line, before the account.
    pub body: String,
}

impl Notification {
    /// The text under the title: the account, then the body.
    pub fn text(&self) -> String {
        format!("{} · {}", self.account_email, self.body)
    }
}
