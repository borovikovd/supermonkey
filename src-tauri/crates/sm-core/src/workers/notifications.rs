//! Showing notifications, and their click-through (SPEC Notifications).
//!
//! The notification plugin does not report clicks on macOS, so a focus soon
//! after a notification shown while the window was in the background counts
//! as a click on it.

use std::sync::Arc;

use parking_lot::Mutex;
use sm_domain::Millis;

use crate::clock::Clock;
use crate::{Notification, Platform};

/// How soon after a background notification a focus counts as its click, ms.
const CLICK_WITHIN_MS: i64 = 30_000;

/// Posts notifications and remembers the last one for click-through. Knows
/// whether the window has focus, for the rule that the focused window's
/// open thread doesn't notify.
pub(crate) struct Notifier {
    platform: Arc<dyn Platform>,
    clock: Arc<dyn Clock>,
    state: Mutex<FocusState>,
}

/// Focus and the last notification, which must agree, under one lock.
#[derive(Default)]
struct FocusState {
    /// When the window lost focus; `None` while it has it.
    away_since: Option<Millis>,
    /// The last notification shown, and when.
    last: Option<(Notification, Millis)>,
}

impl Notifier {
    pub(crate) fn new(platform: Arc<dyn Platform>, clock: Arc<dyn Clock>) -> Self {
        Self {
            platform,
            clock,
            state: Mutex::default(),
        }
    }

    /// Whether the window has focus. It has at launch.
    pub(crate) fn is_focused(&self) -> bool {
        self.state.lock().away_since.is_none()
    }

    /// Posts `notification` and remembers it for click-through.
    pub(crate) fn show(&self, notification: Notification) {
        self.platform.notify(&notification);
        let now = self.clock.now();
        self.state.lock().last = Some((notification, now));
    }

    /// The window gained or lost focus. On focus, returns the notification
    /// the focus counts as a click on: the last one, if it was shown while
    /// the window was away and less than 30 s ago. Either way it is used up.
    pub(crate) fn focus(&self, focused: bool) -> Option<Notification> {
        let now = self.clock.now();
        let mut state = self.state.lock();
        if !focused {
            state.away_since.get_or_insert(now);
            return None;
        }
        let away = state.away_since.take();
        let (notification, at) = state.last.take()?;
        let clicked =
            away.is_some_and(|away| at >= away) && now.0.saturating_sub(at.0) < CLICK_WITHIN_MS;
        clicked.then_some(notification)
    }
}
