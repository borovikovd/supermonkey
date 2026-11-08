//! Mail actions: archive, trash, spam, star, read state, return to inbox,
//! snooze, labels, and undo (SPEC Archive, trash, star, read; Labels;
//! Snooze; Undo). Marking read on open is a `modify` that makes no undo
//! entry (SPEC Reading).

use std::collections::HashSet;

use sm_domain::{
    AccountId, ActionNo, LabelDelta, Millis, ModifyKind, SnoozeEdit, Thread, ThreadChange,
    ThreadId, ThreadModify, UndoEntryId, plan_modify, snooze_after,
};
use sm_ipc::{ModifyAction, SnoozeChange};
use sm_store::{UndoAction, WriteTx};

use super::{ActionCtx, ActionError, splits};
use crate::effects::Effect;
use crate::ops::{ModifyOp, queue_modify, queue_revert, revert};
use crate::writer::WriterState;

/// A label and snooze change to make to some of an account's threads.
pub(crate) struct ThreadsChange {
    /// The threads' account.
    pub(crate) account: AccountId,
    /// What the change is, for its snooze rule and undo label.
    pub(crate) kind: ModifyKind,
    /// The threads; repeats and threads gone since are skipped.
    pub(crate) threads: Vec<ThreadId>,
    /// The labels to add and remove.
    pub(crate) delta: LabelDelta,
    /// How the snooze changes, if it does.
    pub(crate) snooze: Option<SnoozeEdit>,
}

/// `modify`: applies a resolved label and snooze change to each thread's
/// messages as they are now, records what really changed for undo, and
/// queues one `batchModify` naming those messages, so a replay never
/// touches mail that arrives later. The UI resolved any toggle; the webview
/// is not trusted, so every label and thread must be the account's.
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
    let entry = change_threads(ctx.tx, ctx.state, change, Some(ctx.n), ctx.now)?;
    if undoable && !entry.is_empty() {
        ctx.tx.undo().record(
            account,
            UndoEntryId(ctx.n.0),
            kind.undo_label(),
            &UndoAction::Mail { threads: entry },
        )?;
    }
    Ok(())
}

/// Makes `change` here and queues it for Gmail as made by `action` (`None`
/// for the core's own, such as a snooze waking). Returns what really
/// changed, per thread: the undo entry's payload. A snooze time set wakes
/// the snooze timer.
pub(crate) fn change_threads(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    change: ThreadsChange,
    action: Option<ActionNo>,
    now: Millis,
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
            // Gone since the UI saw it, say after a poll.
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
            tx.threads().set_snooze(thread, after)?;
            if after.is_some() {
                state.effect(Effect::Snoozes);
            }
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
        let op = ModifyOp {
            messages,
            add: delta.add().clone(),
            remove: delta.remove().clone(),
            threads: entry.clone(),
        };
        queue_modify(tx, state, account, action, &op, now)?;
    }
    Ok(entry)
}

/// `undo`: reverts one entry of the account's history and removes it. An
/// entry already gone is refused, so two quick undos never undo two entries
/// by mistake (SPEC UI state, Actions). A mail entry reverts only the labels
/// and snooze times it changed, where nothing has changed them since, and
/// queues that revert for Gmail.
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
            queue_revert(ctx.tx, ctx.state, account, ctx.n, reverted, ctx.now)?;
            Ok(())
        }
        UndoAction::Pin {
            pin_kind,
            value,
            split: _,
            replaced,
        } => splits::undo_pin(ctx, account, pin_kind, &value, replaced),
        UndoAction::Send { outbox } => {
            crate::outbox::undo_send(ctx.tx, ctx.state, account, outbox, ctx.now)
        }
    }
}
