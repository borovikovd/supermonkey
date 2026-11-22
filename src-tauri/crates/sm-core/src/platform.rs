use std::path::Path;

use sm_domain::{AccountId, ThreadId};

pub trait Platform: Send + Sync {
    fn open_url(&self, url: &str) -> Result<(), PlatformError>;

    fn open_sign_in(&self, url: &str) -> Result<(), PlatformError>;

    fn open_path(&self, path: &Path) -> Result<(), PlatformError>;

    fn notify(&self, notification: &Notification);

    fn set_badge(&self, count: Option<u32>);

    fn focus_window(&self);
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct PlatformError(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Notification {
    pub account: AccountId,
    pub account_email: String,
    pub thread: Option<ThreadId>,
    pub title: String,
    pub body: String,
}

impl Notification {
    #[must_use]
    pub fn text(&self) -> String {
        format!("{} · {}", self.account_email, self.body)
    }
}
