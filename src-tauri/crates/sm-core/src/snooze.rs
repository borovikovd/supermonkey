//! Waking snoozed threads (SPEC Snooze): on the writer, as the timer, launch,
//! and wake from sleep ask.

use std::collections::BTreeMap;

use sm_domain::{AccountId, LabelDelta, Millis, ModifyKind, SnoozeEdit};
use sm_store::WriteTx;

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
) -> Result<usize, ActionError> {
    let due = tx.read().threads().snoozes_due(now)?;
    let count = due.len();
    let mut by_account: BTreeMap<AccountId, Vec<_>> = BTreeMap::new();
    for (account, thread) in due {
        by_account.entry(account).or_default().push(thread);
    }
    for (account, threads) in by_account {
        let delta = LabelDelta::preset(ModifyKind::ReturnToInbox)
            .ok_or(ActionError::Invalid("Couldn't return that to the inbox"))?;
        let change = ThreadsChange {
            account,
            kind: ModifyKind::ReturnToInbox,
            threads,
            delta,
            snooze: Some(SnoozeEdit::Clear),
        };
        change_threads(tx, state, change, None, now)?;
    }
    if count > 0 {
        tracing::info!(count, "woke due snoozes");
    }
    Ok(count)
}
