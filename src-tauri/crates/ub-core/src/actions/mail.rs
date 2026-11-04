//! Mail actions: archive, trash, spam, star, read state, return to inbox,
//! snooze, labels, and undo (SPEC Archive, trash, star, read; Labels;
//! Snooze; Undo). Marking read on open is a `modify` that makes no undo
//! entry (SPEC Reading).

use std::collections::HashSet;

use ub_domain::{
    AccountId, LabelDelta, SnoozeEdit, Thread, ThreadChange, ThreadModify, UndoEntryId,
    plan_modify, snooze_after,
};
use ub_ipc::{ModifyAction, SnoozeChange};
use ub_store::UndoAction;

use super::{ActionCtx, ActionError, splits};
use crate::ops::{ModifyOp, queue_modify, queue_revert, revert};

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
            return Err(ActionError::Invalid("no such label"));
        }
    }
    let edit = snooze.map(|s| match s {
        SnoozeChange::Set(at) => SnoozeEdit::Set(at),
        SnoozeChange::Clear => SnoozeEdit::Clear,
    });
    let mut seen = HashSet::new();
    let mut entry = Vec::new();
    let mut messages = Vec::new();
    for thread in threads.into_iter().filter(|t| seen.insert(*t)) {
        let Some(row) = ctx.tx.read().threads().row(thread)? else {
            // Gone since the UI saw it, say after a poll.
            continue;
        };
        if row.account != account {
            return Err(ActionError::Invalid("a thread is not in the account"));
        }
        let facts = Thread::new(ctx.tx.read().messages().facts(thread)?);
        let ThreadModify {
            op_messages,
            changes,
        } = plan_modify(&facts, &delta);
        messages.extend(op_messages.into_iter().map(|m| m.gmail_id.clone()));
        ctx.tx.messages().apply(&changes)?;
        let after = snooze_after(edit, kind, row.snooze_until);
        if after != row.snooze_until {
            // Extension point: the snooze slice re-arms its timer here.
            ctx.tx.threads().set_snooze(thread, after)?;
        }
        entry.push(ThreadChange {
            thread,
            changes,
            snooze_before: row.snooze_until,
            snooze_after: after,
        });
    }
    if entry.is_empty() {
        return Ok(());
    }
    let relabels = entry.iter().any(|t| !t.changes.is_empty());
    if relabels && !messages.is_empty() {
        let op = ModifyOp {
            messages,
            add: delta.add().clone(),
            remove: delta.remove().clone(),
            threads: entry.clone(),
        };
        queue_modify(ctx.tx, ctx.state, account, Some(ctx.n), &op, ctx.now)?;
    }
    if undoable {
        ctx.tx.undo().record(
            account,
            UndoEntryId(ctx.n.0),
            kind.undo_label(),
            &UndoAction::Mail { threads: entry },
        )?;
    }
    Ok(())
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
        return Err(ActionError::Invalid("the undo entry is gone"));
    };
    ctx.tx.undo().delete(entry)?;
    match record.action {
        UndoAction::Mail { threads } => {
            let reverted = revert(ctx.tx, &threads)?;
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
