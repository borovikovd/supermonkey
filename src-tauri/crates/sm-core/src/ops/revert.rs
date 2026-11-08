//! Queueing ops, and undoing what an action did here: for `z`, and when
//! Gmail rejects an op (SPEC Undo, Gmail errors).

use sm_domain::{AccountId, ActionNo, LabelId, Millis, NoticeKind, ThreadChange, UndoEntryId};
use sm_store::{NewNotice, NewOp, OpId, OpKind, StoreError, UndoAction, WriteTx};

use super::payload::relabel_changes;
use super::{LabelCreateOp, ModifyOp};
use crate::effects::Effect;
use crate::writer::WriterState;

/// Queues `op`, made by action `action`, and wakes the account's runner
/// once the commit lands.
pub(crate) fn queue_modify(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    action: Option<ActionNo>,
    op: &ModifyOp,
    now: Millis,
) -> Result<(), StoreError> {
    tx.ops().push(&NewOp {
        account,
        action,
        kind: OpKind::Modify,
        draft: None,
        gmail_draft: None,
        payload: op,
        created_at: now,
    })?;
    state.effect(Effect::Ops(account));
    Ok(())
}

/// Reverts `changes` where nothing has changed them since: a message loses
/// what it gained and still has, and gets back what it lost and still
/// lacks; a snooze time nothing has changed since goes back. Later changes,
/// such as a read on open or an edit in another Gmail client, stay. Returns
/// what the revert itself changed, per thread.
pub(crate) fn revert(
    tx: &mut WriteTx<'_>,
    changes: &[ThreadChange],
) -> Result<Vec<ThreadChange>, StoreError> {
    let mut reverted = Vec::with_capacity(changes.len());
    for change in changes {
        let Some(row) = tx.read().threads().row(change.thread)? else {
            continue;
        };
        let mut back = Vec::new();
        for message in &change.changes {
            let Some(current) = tx.read().messages().get(message.message)? else {
                continue;
            };
            back.extend(message.reverted(&current.labels));
        }
        tx.messages().apply(&back)?;
        let now = row.snooze_until;
        let after = if change.restores_snooze(now) {
            tx.threads()
                .set_snooze(change.thread, change.snooze_before)?;
            change.snooze_before
        } else {
            now
        };
        reverted.push(ThreadChange {
            thread: change.thread,
            changes: back,
            snooze_before: now,
            snooze_after: after,
        });
    }
    Ok(reverted)
}

/// Queues the Gmail side of a revert made by action `action`: one
/// `batchModify` per distinct label change, naming the messages that made
/// it. A thread's snooze change rides with its first op, for a rollback.
pub(crate) fn queue_revert(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    action: ActionNo,
    reverted: Vec<ThreadChange>,
    now: Millis,
) -> Result<(), StoreError> {
    let mut ops: Vec<ModifyOp> = Vec::new();
    for thread in reverted {
        let mut snooze = Some((thread.snooze_before, thread.snooze_after));
        for change in thread.changes {
            let Some(message) = tx.read().messages().get(change.message)? else {
                continue;
            };
            let found = ops
                .iter()
                .position(|op| op.add == change.added && op.remove == change.removed);
            let at = if let Some(at) = found {
                at
            } else {
                ops.push(ModifyOp {
                    messages: Vec::new(),
                    add: change.added.clone(),
                    remove: change.removed.clone(),
                    threads: Vec::new(),
                });
                ops.len().saturating_sub(1)
            };
            let Some(op) = ops.get_mut(at) else {
                continue;
            };
            op.messages.push(message.gmail_id);
            if let Some(t) = op.threads.iter_mut().find(|t| t.thread == thread.thread) {
                t.changes.push(change);
            } else {
                // Only one op restores the snooze; the others leave it.
                let (before, after) = snooze
                    .take()
                    .unwrap_or((thread.snooze_after, thread.snooze_after));
                op.threads.push(ThreadChange {
                    thread: thread.thread,
                    changes: vec![change],
                    snooze_before: before,
                    snooze_after: after,
                });
            }
        }
    }
    for op in &ops {
        queue_modify(tx, state, account, Some(action), op, now)?;
    }
    Ok(())
}

/// Gmail rejected op `id` (SPEC Gmail errors): what it did here rolls back,
/// its action's undo entry goes, and a notice with `message` carries Gmail's
/// error to a toast. Replay goes on with the next op. An op already gone
/// (its account removed) changes nothing.
pub(crate) fn reject(
    tx: &mut WriteTx<'_>,
    id: OpId,
    message: &str,
    now: Millis,
) -> Result<(), StoreError> {
    let Some(op) = tx.read().ops().get(id)? else {
        return Ok(());
    };
    match op.kind {
        OpKind::Modify => match op.payload::<ModifyOp>() {
            Ok(payload) => drop(revert(tx, &payload.threads)?),
            Err(e) => {
                tracing::warn!(op = %id, error = %e, "a rejected op does not parse; nothing to roll back");
            }
        },
        OpKind::LabelCreate => match op.payload::<LabelCreateOp>() {
            Ok(payload) => drop_label(tx, op.account, &payload.label)?,
            Err(e) => {
                tracing::warn!(op = %id, error = %e, "a rejected op does not parse; nothing to roll back");
            }
        },
        // Nothing here to roll back: a draft keeps its text when Gmail
        // refuses it, and a sent message trashed by an undo stays trashed
        // here; the notice says Gmail refused.
        OpKind::DraftUpsert | OpKind::DraftDelete | OpKind::TrashMessage => {}
    }
    if let Some(action) = op.action {
        tx.undo().delete(UndoEntryId(action.0))?;
    }
    tx.ops().delete(id)?;
    tx.notices().add(&NewNotice {
        account: op.account,
        kind: NoticeKind::Rejected,
        message: message.to_owned(),
        draft: op.draft,
        thread: None,
        created_at: now,
    })?;
    Ok(())
}

/// A label made here that Gmail refused can't be applied either: the queued
/// changes that use it roll back, newest first, and go with their undo
/// entries; then the label goes.
pub(crate) fn drop_label(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    label: &LabelId,
) -> Result<(), StoreError> {
    let queued = tx.read().ops().of_kind(account, OpKind::Modify)?;
    for op in queued.into_iter().rev() {
        let Ok(payload) = op.payload::<ModifyOp>() else {
            continue;
        };
        if !payload.uses(label) {
            continue;
        }
        revert(tx, &payload.threads)?;
        if let Some(action) = op.action {
            tx.undo().delete(UndoEntryId(action.0))?;
        }
        tx.ops().delete(op.id)?;
    }
    tx.labels().delete_local(account, label)
}

/// A label made here merged into the label Gmail already had under its
/// name: queued ops and undo entries that name the local id name Gmail's
/// label instead.
pub(crate) fn relabel(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    from: &LabelId,
    to: &LabelId,
) -> Result<(), StoreError> {
    for op in tx.read().ops().of_kind(account, OpKind::Modify)? {
        let Ok(mut payload) = op.payload::<ModifyOp>() else {
            continue;
        };
        if payload.relabel(from, to) {
            tx.ops().set_payload(op.id, &payload)?;
        }
    }
    for mut entry in tx.read().undo().entries(account)? {
        let UndoAction::Mail { threads } = &mut entry.action else {
            continue;
        };
        if relabel_changes(threads, from, to) {
            tx.undo()
                .record(account, entry.id, &entry.label, &entry.action)?;
        }
    }
    Ok(())
}
