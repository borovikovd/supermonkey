use sm_domain::{ActionNo, Millis};
use sm_ipc::{Action, Submitted, Unreadable};
use sm_store::{StoreError, WriteTx};

use super::dispatch::{self, ActionCtx};
use super::{ActionError, rejected};
use crate::writer::WriterState;

const UNREADABLE: &str = "Couldn't do that because of a bug in Supermonkey";

pub(crate) fn apply_batch(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    actions: Vec<Submitted>,
    now: Millis,
) -> Result<(), StoreError> {
    let mut last = tx.read().meta().last_action()?;
    for Submitted { n, action } in actions {
        if !is_new(n, last) {
            continue;
        }
        match action {
            Ok(action) => apply_one(tx, state, n, action, now)?,
            Err(unreadable) => refuse_unreadable(tx, state, n, &unreadable)?,
        }
        last = n;
    }
    tx.meta().set_last_action(last)?;
    #[cfg(test)]
    if std::mem::take(&mut state.fail_next_commit) {
        tx.fail_commit();
    }
    Ok(())
}

fn is_new(n: ActionNo, last: ActionNo) -> bool {
    if n > last {
        return true;
    }
    tracing::warn!(
        n = n.0,
        last = last.0,
        "dropping an action at or below the last one applied"
    );
    false
}

fn apply_one(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    n: ActionNo,
    action: Action,
    now: Millis,
) -> Result<(), StoreError> {
    let kind = dispatch::kind(&action);
    let sent = rejected::sent_draft(&action);
    let mark = state.effects_mark();
    state.note_action(n);
    let applied = tx.savepoint(|tx| dispatch::apply(&mut ActionCtx { tx, state, n, now }, action));
    if let Err(e) = applied {
        state.drop_effects_from(mark);
        tracing::warn!(n = n.0, action = kind, error = %e, "action rejected");
        rejected::refuse(tx, state, n, sent, &e)?;
    }
    Ok(())
}

fn refuse_unreadable(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    n: ActionNo,
    unreadable: &Unreadable,
) -> Result<(), StoreError> {
    state.note_action(n);
    tracing::warn!(
        n = n.0,
        action = unreadable.kind.as_deref().unwrap_or("(none)"),
        "refusing an action the core can't read"
    );
    rejected::refuse(tx, state, n, None, &ActionError::Invalid(UNREADABLE))
}
