//! The recording [`Platform`] (ARCHITECTURE.md, section 8.3): what the core
//! asked of the operating system, for tests to assert on, and a browser
//! that completes Google sign-ins against the fake.

use std::path::Path;

use parking_lot::Mutex;
use sm_core::activity::{Tasks, waiting_on_person};
use sm_core::{Notification, Platform, PlatformError};
use url::Url;

/// What the core asked of the OS so far.
#[derive(Debug, Default, Clone)]
pub(crate) struct Recorded {
    pub(crate) notifications: Vec<Notification>,
    /// The Dock badge; `None` when cleared.
    pub(crate) badge: Option<u32>,
    pub(crate) opened_urls: Vec<String>,
    pub(crate) opened_files: Vec<String>,
    pub(crate) focus_requests: usize,
}

/// How the next Google sign-in in the browser goes. Used once.
#[derive(Debug, Default)]
pub(crate) enum SignInMode {
    /// The fake signs in its first user.
    #[default]
    FirstUser,
    /// The fake signs in this user.
    As(String),
    /// The person closes the browser tab: nothing comes back.
    Abandon,
}

/// Records every call. Opening the sign-in page "completes" it with a plain
/// HTTP request: the fake's consent page answers at once and redirects to the
/// core's loopback listener.
pub(crate) struct RecordingPlatform {
    recorded: Mutex<Recorded>,
    sign_in: Mutex<SignInMode>,
    http: reqwest::Client,
    /// The browser's requests, counted as core activity until answered.
    browser: Tasks,
}

impl RecordingPlatform {
    pub(crate) fn new(browser: Tasks) -> Self {
        Self {
            recorded: Mutex::new(Recorded::default()),
            sign_in: Mutex::new(SignInMode::default()),
            http: reqwest::Client::new(),
            browser,
        }
    }

    /// A copy of what was recorded.
    pub(crate) fn recorded(&self) -> Recorded {
        self.recorded.lock().clone()
    }

    /// Forgets what was recorded so far; the badge stays as it is.
    pub(crate) fn clear(&self) {
        let mut r = self.recorded.lock();
        r.notifications.clear();
        r.opened_urls.clear();
        r.opened_files.clear();
        r.focus_requests = 0;
    }

    /// How the next sign-in goes.
    pub(crate) fn next_sign_in(&self, mode: SignInMode) {
        *self.sign_in.lock() = mode;
    }
}

impl Platform for RecordingPlatform {
    fn open_url(&self, url: &str) -> Result<(), PlatformError> {
        self.recorded.lock().opened_urls.push(url.to_owned());
        Ok(())
    }

    fn open_sign_in(&self, url: &str) -> Result<(), PlatformError> {
        self.recorded.lock().opened_urls.push(url.to_owned());
        let mode = std::mem::take(&mut *self.sign_in.lock());
        let consent = match mode {
            SignInMode::Abandon => {
                waiting_on_person();
                return Ok(());
            }
            SignInMode::FirstUser => url.to_owned(),
            SignInMode::As(email) => {
                let mut consent =
                    Url::parse(url).map_err(|e| PlatformError(format!("sign-in page: {e}")))?;
                consent.query_pairs_mut().append_pair("login_hint", &email);
                consent.into()
            }
        };
        let http = self.http.clone();
        self.browser.spawn("browser", async move {
            if let Err(e) = http.get(consent).send().await {
                tracing::warn!(error = %e.without_url(), "the test browser couldn't open the consent page");
            }
        });
        Ok(())
    }

    fn open_path(&self, path: &Path) -> Result<(), PlatformError> {
        self.recorded
            .lock()
            .opened_files
            .push(path.to_string_lossy().into_owned());
        Ok(())
    }

    fn notify(&self, notification: &Notification) {
        self.recorded
            .lock()
            .notifications
            .push(notification.clone());
    }

    fn set_badge(&self, count: Option<u32>) {
        self.recorded.lock().badge = count;
    }

    fn focus_window(&self) {
        self.recorded.lock().focus_requests += 1;
    }
}
