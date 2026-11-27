use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::domain::{AccountId, LabelId, OpId, OpKind, OpQueue, SystemLabel, ThreadChange};
use crate::gmail::{Backoff, Format, Gmail, GmailError, Outcome, Priority as GmailPriority};
use crate::store::{NewNotice, OpRecord, Priority as StorePriority, ReadPool, StoreError, WriteTx};

use super::super::events::Effect;
use super::super::runtime::{Kick, KickGuard, WaitOutcome};
use super::super::writer_state::{
    CreateLabelOpPayload, LandedOp, ModifyOpPayload, TrashSentOpPayload,
};
use super::outbox::execute_draft_op;
use super::worker::AccountSlot;

pub(super) enum OpOutcome {
    Done,
    Retry(GmailError),
    AuthFailed,
}

pub(super) async fn wait_kick(
    kick: &Kick,
    slot: &AccountSlot,
    wait: &mut Option<Duration>,
    cancel: &CancellationToken,
) -> Option<Option<KickGuard>> {
    if let Some(dur) = wait.take() {
        let (outcome, guard) = kick
            .wait_timeout(&slot.clock, dur, &slot.activity, cancel)
            .await;
        if outcome == WaitOutcome::Cancelled {
            return None;
        }
        Some(guard)
    } else {
        kick.wait(&slot.activity, cancel).await.map(Some)
    }
}

pub(super) async fn apply_worker_outcome(
    slot: &Arc<AccountSlot>,
    grant_gen: u64,
    outcome: OpOutcome,
    backoff: &mut Backoff,
    wait: &mut Option<Duration>,
) -> bool {
    match outcome {
        OpOutcome::Done => {
            backoff.reset();
            slot.report_success(grant_gen).await;
            true
        }
        OpOutcome::Retry(err) => {
            *wait = slot.handle_gmail_error(grant_gen, &err, backoff).await;
            false
        }
        OpOutcome::AuthFailed => {
            slot.mark_auth_failed(grant_gen).await;
            false
        }
    }
}

pub(crate) async fn run_op_worker(
    slot: Arc<AccountSlot>,
    queue: OpQueue,
    cancel: CancellationToken,
) {
    let mut backoff = Backoff::new();
    let mut wait: Option<Duration> = None;

    loop {
        if cancel.is_cancelled() {
            return;
        }
        let kick = match queue {
            OpQueue::Mail => &slot.mail_ops_kick,
            OpQueue::Drafts => &slot.draft_ops_kick,
        };
        let Some(guard) = wait_kick(kick, &slot, &mut wait, &cancel).await else {
            return;
        };

        loop {
            if cancel.is_cancelled() {
                drop(guard);
                return;
            }
            let next_op = slot
                .pool
                .with(|db| db.next_op(slot.account, queue))
                .ok()
                .flatten();
            let Some(op) = next_op else {
                backoff.reset();
                break;
            };
            if !slot.is_online() {
                break;
            }
            let Some((grant_gen, gmail)) = slot.gmail() else {
                break;
            };
            let Some(outcome) = cancel
                .run_until_cancelled(async {
                    match queue {
                        OpQueue::Mail => execute_mail_op(&slot, &gmail, &op).await,
                        OpQueue::Drafts => execute_draft_op(&slot, &gmail, &op).await,
                    }
                })
                .await
            else {
                drop(guard);
                return;
            };
            if !apply_worker_outcome(&slot, grant_gen, outcome, &mut backoff, &mut wait).await {
                break;
            }
        }
        drop(guard);
    }
}

pub(super) async fn classify_error(slot: &AccountSlot, op_id: OpId, err: GmailError) -> OpOutcome {
    match err.outcome {
        Outcome::InvalidGrant | Outcome::Unauthorized => OpOutcome::AuthFailed,
        Outcome::Offline
        | Outcome::NoResponse
        | Outcome::RateLimited { .. }
        | Outcome::ServerError { .. } => OpOutcome::Retry(err),
        Outcome::NotFound
        | Outcome::Conflict
        | Outcome::Rejected { .. }
        | Outcome::InvalidRequest(_) => {
            delete_op_now(slot, op_id).await;
            OpOutcome::Done
        }
    }
}

pub(super) async fn delete_op_now(slot: &AccountSlot, op_id: OpId) {
    let acct = slot.account;
    super::super::log_err(
        "delete completed op",
        slot.writer
            .call(StorePriority::Sync, move |tx, _state| {
                tx.delete_op(op_id, acct)
            })
            .await,
    );
}

async fn execute_mail_op(slot: &AccountSlot, gmail: &Gmail, op: &OpRecord) -> OpOutcome {
    match op.kind {
        OpKind::Modify => execute_modify_op(slot, gmail, op).await,
        OpKind::CreateLabel => execute_create_label_op(slot, gmail, op).await,
        OpKind::TrashMessage => execute_trash_op(slot, gmail, op).await,
        OpKind::DraftUpsert | OpKind::DraftDelete => {
            delete_op_now(slot, op.id).await;
            OpOutcome::Done
        }
    }
}

async fn execute_modify_op(slot: &AccountSlot, gmail: &Gmail, op: &OpRecord) -> OpOutcome {
    let Ok(payload) = op.parse_payload::<ModifyOpPayload>() else {
        delete_op_now(slot, op.id).await;
        return OpOutcome::Done;
    };
    let Some((add_wire, remove_wire)) =
        resolve_wire_labels(&slot.pool, slot.account, &payload.add, &payload.remove)
    else {
        delete_op_now(slot, op.id).await;
        return OpOutcome::Done;
    };
    if payload.gmail_msg_ids.is_empty() || (add_wire.is_empty() && remove_wire.is_empty()) {
        delete_op_now(slot, op.id).await;
        return OpOutcome::Done;
    }

    match gmail
        .batch_modify(
            &payload.gmail_msg_ids,
            &add_wire,
            &remove_wire,
            GmailPriority::P1,
        )
        .await
    {
        Ok(()) => {
            finish_modify_success(slot, op.id, payload).await;
            OpOutcome::Done
        }
        Err(err) if err.outcome == Outcome::NotFound => {
            recover_modify_not_found(slot, gmail, op, payload, &add_wire, &remove_wire).await
        }
        Err(err) if err.is_transient() => OpOutcome::Retry(err),
        Err(err) if matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized) => {
            OpOutcome::AuthFailed
        }
        Err(err) => {
            revert_rejected_modify(slot, op.id, payload, &err.detail).await;
            OpOutcome::Done
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    clippy::cognitive_complexity,
    reason = "probes each message individually on 404, prunes deleted messages, and retries batchModify on survivors"
)]
async fn recover_modify_not_found(
    slot: &AccountSlot,
    gmail: &Gmail,
    op: &OpRecord,
    payload: ModifyOpPayload,
    add_wire: &[LabelId],
    remove_wire: &[LabelId],
) -> OpOutcome {
    let mut survivors = Vec::new();
    let mut missing = Vec::new();
    for gid in &payload.gmail_msg_ids {
        match gmail
            .get_message(gid, Format::Minimal, GmailPriority::P1)
            .await
        {
            Err(err) if err.outcome == Outcome::NotFound => missing.push(gid.clone()),
            Err(err) if err.is_transient() => return OpOutcome::Retry(err),
            Err(err) if matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized) => {
                return OpOutcome::AuthFailed;
            }
            Ok(_) | Err(_) => survivors.push(gid.clone()),
        }
    }

    if !missing.is_empty() {
        let acct = slot.account;
        super::super::log_err(
            "prune missing messages after 404 probe",
            slot.writer
                .call(StorePriority::Sync, move |tx, state| {
                    for gid in &missing {
                        if let Some(msg) = tx.read().message_by_gmail(acct, gid)? {
                            tx.delete_message(msg.id)?;
                        }
                    }
                    state.emit(Effect::RefreshBadge);
                    Ok::<_, StoreError>(())
                })
                .await,
        );
    }

    if survivors.is_empty() {
        delete_op_now(slot, op.id).await;
        return OpOutcome::Done;
    }

    match gmail
        .batch_modify(&survivors, add_wire, remove_wire, GmailPriority::P1)
        .await
    {
        Ok(()) => {
            let mut adjusted = payload;
            adjusted.gmail_msg_ids = survivors;
            finish_modify_success(slot, op.id, adjusted).await;
            OpOutcome::Done
        }
        Err(err) if err.is_transient() => OpOutcome::Retry(err),
        Err(err) if matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized) => {
            OpOutcome::AuthFailed
        }
        Err(err) => {
            revert_rejected_modify(slot, op.id, payload, &err.detail).await;
            OpOutcome::Done
        }
    }
}

async fn finish_modify_success(slot: &AccountSlot, op_id: OpId, payload: ModifyOpPayload) {
    let acct = slot.account;
    super::super::log_err(
        "record completed modify op",
        slot.writer
            .call(StorePriority::Sync, move |tx, state| {
                let rev = tx.stamp();
                tx.delete_op(op_id, acct)?;
                state.freshness.record_landed(
                    acct,
                    LandedOp {
                        gmail_msg_ids: payload.gmail_msg_ids,
                        add: payload.add,
                        remove: payload.remove,
                        at_rev: rev,
                        min_history_id: None,
                    },
                );
                Ok::<_, StoreError>(())
            })
            .await,
    );
}

async fn revert_rejected_modify(
    slot: &AccountSlot,
    op_id: OpId,
    payload: ModifyOpPayload,
    reason: &str,
) {
    let acct = slot.account;
    let msg = format!("Couldn't save that change to Gmail: {reason}");
    super::super::log_err(
        "revert rejected modify op",
        slot.writer
            .call(StorePriority::Sync, move |tx, state| {
                tx.delete_op(op_id, acct)?;
                revert_thread_change(tx, acct, &payload.revert)?;
                tx.add_notice(&NewNotice {
                    account: acct,
                    kind: crate::domain::NoticeKind::Rejected,
                    message: &msg,
                    draft: None,
                    thread: Some(payload.thread),
                })?;
                state.emit(Effect::KickSnooze);
                state.emit(Effect::RefreshBadge);
                Ok::<_, StoreError>(())
            })
            .await,
    );
}

fn revert_thread_change(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    change: &ThreadChange,
) -> Result<(), StoreError> {
    let Some(row) = tx.read().thread_row(change.thread)? else {
        return Ok(());
    };
    if row.account != account {
        return Ok(());
    }
    let msgs = tx.read().thread_messages(change.thread)?;
    let by_id: HashMap<_, _> = msgs.iter().map(|m| (m.id, m)).collect();
    for mc in &change.messages {
        if let Some(msg) = by_id.get(&mc.message)
            && let Some(rev) = mc.reverted(&msg.labels)
        {
            let next = rev.apply(&msg.labels);
            tx.set_message_labels(msg.id, &next)?;
        }
    }
    tx.set_thread_snooze(change.thread, change.snooze_undone(row.snooze_until))?;
    tx.derive_thread(change.thread)?;
    Ok(())
}

fn resolve_wire_labels(
    pool: &ReadPool,
    account: AccountId,
    add: &[LabelId],
    remove: &[LabelId],
) -> Option<(Vec<LabelId>, Vec<LabelId>)> {
    let labels = pool.with(|db| db.labels(account)).ok()?;
    let map: HashMap<LabelId, Option<LabelId>> =
        labels.into_iter().map(|l| (l.id, l.gmail_id)).collect();
    let translate = |list: &[LabelId]| -> Vec<LabelId> {
        list.iter()
            .filter_map(|id| {
                if SystemLabel::is_system_id(id.as_str()) {
                    Some(id.clone())
                } else {
                    map.get(id).and_then(Clone::clone)
                }
            })
            .collect()
    };
    Some((translate(add), translate(remove)))
}

#[expect(
    clippy::too_many_lines,
    reason = "handles label creation, 409 conflict reconciliation, and 4xx rollback with notice"
)]
async fn execute_create_label_op(slot: &AccountSlot, gmail: &Gmail, op: &OpRecord) -> OpOutcome {
    let Ok(payload) = op.parse_payload::<CreateLabelOpPayload>() else {
        delete_op_now(slot, op.id).await;
        return OpOutcome::Done;
    };
    let created = match gmail.create_label(&payload.name, GmailPriority::P1).await {
        Ok(lbl) => Ok(lbl),
        Err(err) if err.outcome == Outcome::Conflict => {
            match gmail.list_labels(GmailPriority::P1).await {
                Ok(all) => {
                    if let Some(found) = all
                        .into_iter()
                        .find(|l| l.name.eq_ignore_ascii_case(&payload.name))
                    {
                        Ok(found)
                    } else {
                        Err(err)
                    }
                }
                Err(list_err) => Err(list_err),
            }
        }
        Err(err) => Err(err),
    };

    match created {
        Ok(lbl) => {
            let acct = slot.account;
            let op_id = op.id;
            let local_id = payload.local_id;
            super::super::log_err(
                "assign Gmail label id",
                slot.writer
                    .call(StorePriority::Sync, move |tx, state| {
                        tx.delete_op(op_id, acct)?;
                        let canonical =
                            tx.assign_gmail_label(acct, &local_id, &lbl.id, &lbl.name)?;
                        if canonical != local_id {
                            rewrite_queued_ops_label(tx, acct, &local_id, &canonical)?;
                        }
                        state.freshness.bump_labels(acct);
                        Ok::<_, StoreError>(())
                    })
                    .await,
            );
            OpOutcome::Done
        }
        Err(err) if err.is_transient() => OpOutcome::Retry(err),
        Err(err) if matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized) => {
            OpOutcome::AuthFailed
        }
        Err(err) => {
            let acct = slot.account;
            let op_id = op.id;
            let local_id = payload.local_id;
            let msg = format!("Couldn't create label {}: {}", payload.name, err.detail);
            super::super::log_err(
                "roll back failed label creation",
                slot.writer
                    .call(StorePriority::Sync, move |tx, state| {
                        tx.delete_op(op_id, acct)?;
                        rollback_failed_label(tx, acct, &local_id)?;
                        tx.add_notice(&NewNotice {
                            account: acct,
                            kind: crate::domain::NoticeKind::Rejected,
                            message: &msg,
                            draft: None,
                            thread: None,
                        })?;
                        state.freshness.bump_labels(acct);
                        Ok::<_, StoreError>(())
                    })
                    .await,
            );
            OpOutcome::Done
        }
    }
}

fn rewrite_queued_ops_label(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    from: &LabelId,
    to: &LabelId,
) -> Result<(), StoreError> {
    let ops = tx.read().all_ops(account)?;
    for op in ops {
        if op.kind != OpKind::Modify {
            continue;
        }
        let Ok(mut payload) = op.parse_payload::<ModifyOpPayload>() else {
            continue;
        };
        let mut changed = false;
        for l in payload.add.iter_mut().chain(payload.remove.iter_mut()) {
            if l == from {
                *l = to.clone();
                changed = true;
            }
        }
        if changed {
            tx.update_op_payload(op.id, account, &payload)?;
        }
    }
    Ok(())
}

fn rollback_failed_label(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    local_id: &LabelId,
) -> Result<(), StoreError> {
    let ops = tx.read().all_ops(account)?;
    for op in ops {
        if op.kind != OpKind::Modify {
            continue;
        }
        let Ok(mut payload) = op.parse_payload::<ModifyOpPayload>() else {
            continue;
        };
        let had = payload.add.iter().any(|l| l == local_id)
            || payload.remove.iter().any(|l| l == local_id);
        if had {
            payload.add.retain(|l| l != local_id);
            payload.remove.retain(|l| l != local_id);
            if payload.add.is_empty() && payload.remove.is_empty() {
                tx.delete_op(op.id, account)?;
            } else {
                tx.update_op_payload(op.id, account, &payload)?;
            }
            if let Some(row) = tx.read().thread_row(payload.thread)?
                && row.account == account
            {
                let msgs = tx.read().thread_messages(payload.thread)?;
                for msg in msgs {
                    if msg.labels.contains(local_id.as_str()) {
                        let next = msg.labels.without(std::slice::from_ref(local_id));
                        tx.set_message_labels(msg.id, &next)?;
                    }
                }
                tx.derive_thread(payload.thread)?;
            }
        }
    }
    tx.delete_label(account, local_id)?;
    Ok(())
}

async fn execute_trash_op(slot: &AccountSlot, gmail: &Gmail, op: &OpRecord) -> OpOutcome {
    let Ok(payload) = op.parse_payload::<TrashSentOpPayload>() else {
        delete_op_now(slot, op.id).await;
        return OpOutcome::Done;
    };
    match gmail
        .trash_message(&payload.gmail_msg_id, GmailPriority::P1)
        .await
    {
        Ok(_) => {
            delete_op_now(slot, op.id).await;
            OpOutcome::Done
        }
        Err(err) if err.outcome == Outcome::NotFound => {
            delete_op_now(slot, op.id).await;
            OpOutcome::Done
        }
        Err(err) => classify_error(slot, op.id, err).await,
    }
}
