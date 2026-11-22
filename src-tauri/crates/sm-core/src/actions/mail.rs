use std::collections::HashSet;

use sm_domain::{
    AccountId, ActionNo, GmailMessageId, LabelDelta, Millis, ModifyKind, SnoozeEdit, Thread,
    ThreadChange, ThreadId, ThreadModify, UndoEntryId, plan_modify, snooze_after,
};
use sm_ipc::{ModifyAction, SnoozeChange};
use sm_store::{StoreError, UndoAction, WriteTx};

use super::{ActionCtx, ActionError, splits};
use crate::effects::Effect;
use crate::ops::{ModifyOp, OpOrigin, queue_modify, queue_revert, revert};
use crate::writer::WriterState;

pub(crate) struct ThreadsChange {
    pub(crate) account: AccountId,
    pub(crate) kind: ModifyKind,
    pub(crate) threads: Vec<ThreadId>,
    pub(crate) delta: LabelDelta,
    pub(crate) snooze: Option<SnoozeEdit>,
}

pub(super) fn modify(ctx: &mut ActionCtx<'_, '_>, action: ModifyAction) -> Result<(), ActionError> {
    let ModifyAction {
        account,
        kind,
        threads,
        add,
        remove,
        snooze,
        undoable,
    } = action;
    let delta = LabelDelta::new(add.into_iter().collect(), remove.into_iter().collect())?;
    for label in delta.add().iter().chain(delta.remove()) {
        if ctx.tx.read().labels().get(account, label)?.is_none() {
            return Err(ActionError::Invalid("That label no longer exists"));
        }
    }
    let snooze = snooze.map(|s| match s {
        SnoozeChange::Set(at) => SnoozeEdit::Set(at),
        SnoozeChange::Clear => SnoozeEdit::Clear,
    });
    let change = ThreadsChange {
        account,
        kind,
        threads,
        delta,
        snooze,
    };
    let entry = change_threads(ctx.tx, ctx.state, change, Some(ctx.n))?;
    if undoable && !entry.is_empty() {
        ctx.tx.undo().record(
            account,
            UndoEntryId(ctx.n.0),
            &UndoAction::Mail { threads: entry },
        )?;
    }
    Ok(())
}

pub(crate) fn change_threads(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    change: ThreadsChange,
    action: Option<ActionNo>,
) -> Result<Vec<ThreadChange>, ActionError> {
    let ThreadsChange {
        account,
        kind,
        threads,
        delta,
        snooze,
    } = change;
    let mut seen = HashSet::new();
    let mut entry = Vec::new();
    let mut messages = Vec::new();
    for thread in threads.into_iter().filter(|t| seen.insert(*t)) {
        let Some(row) = tx.read().threads().row(thread)? else {
            continue;
        };
        if row.account != account {
            return Err(ActionError::Invalid(
                "A conversation is no longer in this account",
            ));
        }
        let facts = Thread::new(tx.read().messages().facts(thread)?);
        let ThreadModify {
            op_messages,
            changes,
        } = plan_modify(&facts, &delta);
        messages.extend(op_messages.into_iter().map(|m| m.gmail_id.clone()));
        tx.messages().apply(&changes)?;
        let after = snooze_after(snooze, kind, row.snooze_until);
        if after != row.snooze_until {
            set_snooze(tx, state, thread, after)?;
        }
        entry.push(ThreadChange {
            thread,
            changes,
            snooze_before: row.snooze_until,
            snooze_after: after,
        });
    }
    let relabels = entry.iter().any(|t| !t.changes.is_empty());
    if relabels && !messages.is_empty() {
        let op = relabel_op(delta, messages, entry);
        let origin = OpOrigin { account, action };
        queue_modify(tx, state, origin, &op)?;
        return Ok(op.threads);
    }
    Ok(entry)
}

fn relabel_op(
    delta: LabelDelta,
    messages: Vec<GmailMessageId>,
    threads: Vec<ThreadChange>,
) -> ModifyOp {
    let (add, remove) = delta.into_parts();
    ModifyOp {
        messages,
        add,
        remove,
        threads,
    }
}

fn set_snooze(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    thread: ThreadId,
    until: Option<Millis>,
) -> Result<(), StoreError> {
    tx.threads().set_snooze(thread, until)?;
    if until.is_some() {
        state.effect(Effect::Snoozes);
    }
    Ok(())
}

pub(super) fn undo(
    ctx: &mut ActionCtx<'_, '_>,
    account: AccountId,
    entry: UndoEntryId,
) -> Result<(), ActionError> {
    let Some(record) = ctx.tx.read().undo().get(account, entry)? else {
        return Err(ActionError::Invalid("That change can no longer be undone"));
    };
    ctx.tx.undo().delete(entry)?;
    match record.action {
        UndoAction::Mail { threads } => {
            let reverted = revert(ctx.tx, &threads)?;
            if reverted
                .iter()
                .any(|t| t.snooze_after.is_some() && t.snooze_after != t.snooze_before)
            {
                ctx.state.effect(Effect::Snoozes);
            }
            let origin = OpOrigin {
                account,
                action: Some(ctx.n),
            };
            queue_revert(ctx.tx, ctx.state, origin, reverted)?;
            Ok(())
        }
        UndoAction::Pin {
            pin_kind,
            value,
            split: _,
            replaced,
        } => splits::undo_pin(ctx, account, pin_kind, &value, replaced),
        UndoAction::Send { outbox } => crate::outbox::undo_send(ctx.tx, ctx.state, account, outbox),
    }
}
