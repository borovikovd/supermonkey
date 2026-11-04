//! Waking snoozed threads (SPEC Snooze): on the writer, as the timer, launch,
//! and wake from sleep ask.

use std::collections::BTreeMap;

use ub_domain::{AccountId, LabelDelta, LabelSet, Millis, ModifyKind, SnoozeEdit, SystemLabel};
use ub_store::{StoreError, WriteTx};

use crate::actions::{ActionError, ThreadsChange, change_threads};
use crate::writer::WriterState;

/// Wakes every snooze due by `now`: the thread gets `INBOX` back and loses
/// its snooze here at once, and the label change is queued for Gmail, where
/// it waits while Gmail can't be reached. A wake is not an undo entry.
/// Returns how many woke.
pub(crate) fn wake_due(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    now: Millis,
) -> Result<usize, StoreError> {
    let due = tx.read().threads().snoozes_due(now)?;
    let count = due.len();
    let mut by_account: BTreeMap<AccountId, Vec<_>> = BTreeMap::new();
    for (account, thread) in due {
        by_account.entry(account).or_default().push(thread);
    }
    for (account, threads) in by_account {
        let mut add = LabelSet::new();
        add.insert(SystemLabel::Inbox.label_id());
        let delta = LabelDelta::new(add, LabelSet::new())
            .map_err(|e| StoreError::Invalid(e.to_string()))?;
        let change = ThreadsChange {
            account,
            kind: ModifyKind::ReturnToInbox,
            threads,
            delta,
            snooze: Some(SnoozeEdit::Clear),
        };
        change_threads(tx, state, change, None, now).map_err(|e| match e {
            ActionError::Store(e) => e,
            other => StoreError::Invalid(other.to_string()),
        })?;
    }
    if count > 0 {
        tracing::info!(count, "woke due snoozes");
    }
    Ok(count)
}
