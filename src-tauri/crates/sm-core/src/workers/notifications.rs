use std::sync::Arc;

use parking_lot::Mutex;
use sm_domain::Millis;

use crate::clock::Clock;
use crate::{Notification, Platform};

const CLICK_WITHIN_MS: i64 = 30_000;

pub(crate) struct Notifier {
    platform: Arc<dyn Platform>,
    clock: Arc<dyn Clock>,
    state: Mutex<FocusState>,
}

#[derive(Default)]
struct FocusState {
    away_since: Option<Millis>,
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

    pub(crate) fn is_focused(&self) -> bool {
        self.state.lock().away_since.is_none()
    }

    pub(crate) fn show(&self, notification: Notification) {
        self.platform.notify(&notification);
        let now = self.clock.now();
        self.state.lock().last = Some((notification, now));
    }

    pub(crate) fn focus(&self, focused: bool) -> Option<Notification> {
        let now = self.clock.now();
        let mut state = self.state.lock();
        if !focused {
            state.away_since.get_or_insert(now);
            return None;
        }
        let away = state.away_since.take();
        let (notification, at) = state.last.take()?;
        let clicked = away.is_some_and(|away| at >= away) && now.since(at) < CLICK_WITHIN_MS;
        clicked.then_some(notification)
    }
}
