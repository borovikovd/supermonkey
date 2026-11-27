use std::collections::HashSet;

use parking_lot::Mutex;

use crate::domain::{AccountId, Millis, SettingKey, ThreadId};
use crate::store::{ReadPool, StoreError};

use super::events::NewMailAlert;
use super::runtime::Clock;
use super::{Notification, Platform};

const NOTIFICATION_CLICK_WINDOW_MS: i64 = 30_000;

#[derive(Debug)]
struct NotifyState {
    focused: bool,
    last_alert: Option<(Millis, AccountId, Option<ThreadId>)>,
}

#[derive(Debug)]
pub(crate) struct NotificationTracker {
    state: Mutex<NotifyState>,
}

impl Default for NotificationTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationTracker {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            state: Mutex::new(NotifyState {
                focused: true,
                last_alert: None,
            }),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "notification dispatch evaluates open threads, mute flag, focus state, clock, and platform"
    )]
    pub(crate) fn notify_new_mail(
        &self,
        account: AccountId,
        email: &str,
        items: &[NewMailAlert],
        open_threads: &HashSet<ThreadId>,
        muted: bool,
        clock: &Clock,
        platform: &dyn Platform,
    ) {
        if muted || items.is_empty() {
            return;
        }
        let focused = self.state.lock().focused;
        let visible: Vec<&NewMailAlert> = items
            .iter()
            .filter(|i| !(focused && open_threads.contains(&i.thread_id)))
            .collect();
        if visible.is_empty() {
            return;
        }
        if visible.len() > 3 {
            platform.notify(Notification {
                account: email.to_owned(),
                thread: None,
                title: format!("{} new messages", visible.len()),
                body: format!("New mail in {email}"),
            });
            self.record_alert_if_unfocused(clock.now(), account, None);
            return;
        }
        let mut last_tid = None;
        for item in visible {
            let body = if item.snippet.is_empty() {
                format!("{} — {email}", item.subject)
            } else {
                format!("{} — {} ({email})", item.subject, item.snippet)
            };
            platform.notify(Notification {
                account: email.to_owned(),
                thread: Some(item.thread_id),
                title: item.sender.clone(),
                body,
            });
            last_tid = Some(item.thread_id);
        }
        self.record_alert_if_unfocused(clock.now(), account, last_tid);
    }

    fn record_alert_if_unfocused(&self, at: Millis, account: AccountId, thread: Option<ThreadId>) {
        let mut st = self.state.lock();
        if !st.focused {
            st.last_alert = Some((at, account, thread));
        }
    }

    pub(crate) fn set_focused(
        &self,
        focused: bool,
        clock: &Clock,
    ) -> Option<(AccountId, Option<ThreadId>)> {
        let entry = {
            let mut st = self.state.lock();
            let was_focused = std::mem::replace(&mut st.focused, focused);
            if focused && !was_focused {
                st.last_alert.take()
            } else {
                None
            }
        };
        if let Some((at, account, thread)) = entry
            && clock.now().0.saturating_sub(at.0) <= NOTIFICATION_CLICK_WINDOW_MS
        {
            return Some((account, thread));
        }
        None
    }
}

pub(crate) fn refresh_badge(pool: &ReadPool, platform: &dyn Platform) {
    let total = pool
        .with(|db| {
            let mut sum = 0u32;
            for acct in db.accounts()? {
                if !db.setting_is_on(Some(acct.id), SettingKey::Muted)? {
                    sum = sum.saturating_add(db.inbox_unread(acct.id)?);
                }
            }
            Ok::<_, StoreError>(sum)
        })
        .unwrap_or(0);
    platform.set_badge(total);
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::*;
    use crate::core::CoreError;

    struct FocusOnNotify {
        tracker: Arc<NotificationTracker>,
        clock: Clock,
    }

    impl Platform for FocusOnNotify {
        fn open_url(&self, _url: &str) -> Result<(), CoreError> {
            Ok(())
        }

        fn open_file(&self, _path: &Path) -> Result<(), CoreError> {
            Ok(())
        }

        fn notify(&self, _notification: Notification) {
            let _ = self.tracker.set_focused(true, &self.clock);
        }

        fn set_badge(&self, _count: u32) {}
    }

    #[test]
    fn focus_gained_during_platform_notify_does_not_leave_stale_last_alert() {
        let clock = Clock::manual(Millis(1_000));
        let tracker = Arc::new(NotificationTracker::new());
        let _ = tracker.set_focused(false, &clock);
        let platform = FocusOnNotify {
            tracker: Arc::clone(&tracker),
            clock: clock.clone(),
        };
        let alert = NewMailAlert {
            thread_id: ThreadId(42),
            gmail_thread_id: crate::domain::GmailThreadId("gt-42".to_owned()),
            sender: "Bob".to_owned(),
            subject: "Hi".to_owned(),
            snippet: "Body".to_owned(),
        };
        tracker.notify_new_mail(
            AccountId(1),
            "ana@acme.test",
            &[alert],
            &HashSet::new(),
            false,
            &clock,
            &platform,
        );
        let _ = tracker.set_focused(false, &clock);
        assert_eq!(
            tracker.set_focused(true, &clock),
            None,
            "focus gained during platform.notify must not record a stale last_alert"
        );
    }
}
