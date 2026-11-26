use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use crate::domain::{AccountId, Millis, SettingKey, ThreadId};
use crate::store::{ReadPool, StoreError};

use super::events::NewMailAlert;
use super::runtime::Clock;
use super::{Notification, Platform};

const NOTIFICATION_CLICK_WINDOW_MS: i64 = 30_000;

#[derive(Debug)]
pub(crate) struct NotificationTracker {
    focused: AtomicBool,
    last_alert: Mutex<Option<(Millis, AccountId, Option<ThreadId>)>>,
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
            focused: AtomicBool::new(true),
            last_alert: Mutex::new(None),
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
        let focused = self.focused.load(Ordering::Acquire);
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
            if !focused {
                *self.last_alert.lock() = Some((clock.now(), account, None));
            }
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
        if !focused {
            *self.last_alert.lock() = Some((clock.now(), account, last_tid));
        }
    }

    pub(crate) fn set_focused(
        &self,
        focused: bool,
        clock: &Clock,
    ) -> Option<(AccountId, Option<ThreadId>)> {
        let was_focused = self.focused.swap(focused, Ordering::AcqRel);
        if focused && !was_focused {
            let entry = self.last_alert.lock().take();
            if let Some((at, account, thread)) = entry
                && clock.now().0.saturating_sub(at.0) <= NOTIFICATION_CLICK_WINDOW_MS
            {
                return Some((account, thread));
            }
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
