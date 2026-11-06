//! A batch of numbered actions, applied in one writer job.

use ub_domain::Millis;
use ub_ipc::NumberedAction;
use ub_store::{StoreError, WriteTx};

use super::dispatch::{self, ActionCtx};
use super::rejected;
use crate::writer::WriterState;

/// Applies each action numbered above the last one applied, in order, each
/// in its own savepoint, then stores the last number. A rejected action
/// rolls back alone, its effects dropped, and leaves a notice saying why;
/// the batch goes on and `ack` moves past it. An action at or below the last
/// one applied was already applied or rejected, so it is dropped with no
/// notice. `now` is when the batch was submitted.
pub(crate) fn apply_batch(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    actions: Vec<NumberedAction>,
    now: Millis,
) -> Result<(), StoreError> {
    let mut last = tx.read().meta().last_action()?;
    for NumberedAction { n, action } in actions {
        // Numbers never go back: a reloaded page asks for the last one first.
        if n <= last {
            tracing::warn!(
                n = n.0,
                last = last.0,
                "dropping an action at or below the last one applied"
            );
            continue;
        }
        let kind = dispatch::kind(&action);
        let owner = rejected::owner(&action);
        let mark = state.effects_mark();
        let applied =
            tx.savepoint(|tx| dispatch::apply(&mut ActionCtx { tx, state, n, now }, action));
        if let Err(e) = applied {
            state.drop_effects_from(mark);
            tracing::warn!(n = n.0, action = kind, error = %e, "action rejected");
            rejected::notify(tx, owner, &e, now)?;
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
