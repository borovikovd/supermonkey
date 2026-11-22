use std::collections::BTreeMap;

use sm_domain::{AccountId, LabelDelta, Millis, ModifyKind, SnoozeEdit, ThreadId};
use sm_store::WriteTx;

use crate::actions::{ActionError, ThreadsChange, change_threads};
use crate::writer::WriterState;

pub(crate) fn wake_due(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    now: Millis,
) -> Result<usize, ActionError> {
    let due = tx.read().threads().snoozes_due(now)?;
    let count = due.len();
    for (account, threads) in by_account(due) {
        let delta = LabelDelta::preset(ModifyKind::ReturnToInbox)
            .ok_or(ActionError::Invalid("Couldn't return that to the inbox"))?;
        let change = ThreadsChange {
            account,
            kind: ModifyKind::ReturnToInbox,
            threads,
            delta,
            snooze: Some(SnoozeEdit::Clear),
        };
        change_threads(tx, state, change, None)?;
    }
    if count > 0 {
        tracing::info!(count, "woke due snoozes");
    }
    Ok(count)
}

fn by_account(due: Vec<(AccountId, ThreadId)>) -> BTreeMap<AccountId, Vec<ThreadId>> {
    let mut by_account: BTreeMap<AccountId, Vec<ThreadId>> = BTreeMap::new();
    for (account, thread) in due {
        by_account.entry(account).or_default().push(thread);
    }
    by_account
}
