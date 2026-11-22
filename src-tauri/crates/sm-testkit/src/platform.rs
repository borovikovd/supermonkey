use std::path::Path;

use parking_lot::Mutex;
use sm_core::activity::{Tasks, waiting_on_person};
use sm_core::{Notification, Platform, PlatformError};
use url::Url;

#[derive(Debug, Default, Clone)]
pub(crate) struct Recorded {
    pub(crate) notifications: Vec<Notification>,
    pub(crate) badge: Option<u32>,
    pub(crate) opened_urls: Vec<String>,
    pub(crate) opened_files: Vec<String>,
    pub(crate) focus_requests: usize,
}

#[derive(Debug, Default)]
pub(crate) enum SignInMode {
    #[default]
    FirstUser,
    As(String),
    Abandon,
}

pub(crate) struct RecordingPlatform {
    recorded: Mutex<Recorded>,
    sign_in: Mutex<SignInMode>,
    http: reqwest::Client,
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

    pub(crate) fn recorded(&self) -> Recorded {
        self.recorded.lock().clone()
    }

    pub(crate) fn clear(&self) {
        let mut r = self.recorded.lock();
        r.notifications.clear();
        r.opened_urls.clear();
        r.opened_files.clear();
        r.focus_requests = 0;
    }

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
