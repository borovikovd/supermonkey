//! macOS notifications and the Dock badge (SPEC Notifications).

use crate::model::*;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Notification {
    pub account: AccountId,
    pub account_email: String,
    /// None for a summary of several messages.
    pub thread: Option<ThreadId>,
    pub title: String,
    pub body: String,
}
