//! macOS notifications and the Dock badge (SPEC Notifications).

use crate::model::{AccountId, ThreadId};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Notification {
    pub account: AccountId,
    pub account_email: String,
    /// None for a summary of several messages.
    pub thread: Option<ThreadId>,
    pub title: String,
    pub body: String,
}

impl Notification {
    /// The text under the title: the account, then the body.
    pub fn text(&self) -> String {
        format!("{} · {}", self.account_email, self.body)
    }
}
