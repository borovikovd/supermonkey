use serde::Serialize;
use serde::de::DeserializeOwned;
use sm_domain::{AccountId, ActionNo, LabelId, NoticeKind, ThreadChange, UndoEntryId};
use sm_store::{NewNotice, NewOp, OpId, OpKind, OpRecord, StoreError, UndoAction, WriteTx};

use super::payload::relabel_changes;
use super::{LabelCreateOp, ModifyOp};
use crate::effects::Effect;
use crate::writer::WriterState;

#[derive(Debug, Clone, Copy)]
pub(crate) struct OpOrigin {
    pub(crate) account: AccountId,
    pub(crate) action: Option<ActionNo>,
}

pub(crate) fn queue_op<P: Serialize>(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    op: &NewOp<'_, P>,
) -> Result<(), StoreError> {
    tx.ops().push(op)?;
    state.effect(Effect::Ops(op.account));
    Ok(())
}

pub(crate) fn queue_modify(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    origin: OpOrigin,
    op: &ModifyOp,
) -> Result<(), StoreError> {
    let OpOrigin { account, action } = origin;
    queue_op(
        tx,
        state,
        &NewOp {
            account,
            action,
            kind: OpKind::Modify,
            draft: None,
            gmail_draft: None,
            payload: op,
        },
    )
}

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

pub(crate) fn queue_revert(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    origin: OpOrigin,
    reverted: Vec<ThreadChange>,
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
        queue_modify(tx, state, origin, op)?;
    }
    Ok(())
}

pub(crate) fn reject(tx: &mut WriteTx<'_>, id: OpId, message: String) -> Result<(), StoreError> {
    let Some(op) = tx.read().ops().get(id)? else {
        return Ok(());
    };
    match op.kind {
        OpKind::Modify => {
            if let Some(payload) = rejected_payload::<ModifyOp>(&op) {
                drop(revert(tx, &payload.threads)?);
            }
        }
        OpKind::LabelCreate => {
            if let Some(payload) = rejected_payload::<LabelCreateOp>(&op) {
                drop_label(tx, op.account, &payload.label)?;
            }
        }
        OpKind::DraftUpsert | OpKind::DraftDelete | OpKind::TrashMessage => {}
    }
    if let Some(action) = op.action {
        tx.undo().delete(UndoEntryId(action.0))?;
    }
    tx.ops().delete(id)?;
    tx.notices().add(&NewNotice {
        account: op.account,
        kind: NoticeKind::Rejected,
        message,
        draft: op.draft,
        thread: None,
    })?;
    Ok(())
}

fn rejected_payload<P: DeserializeOwned>(op: &OpRecord) -> Option<P> {
    op.payload::<P>()
        .inspect_err(|e| {
            tracing::warn!(op = %op.id, error = %e, "a rejected op does not parse; nothing to roll back");
        })
        .ok()
}

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
            tx.undo().record(account, entry.id, &entry.action)?;
        }
    }
    Ok(())
}
