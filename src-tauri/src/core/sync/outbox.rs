use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::domain::{
    AccountId, DraftId, GmailDraftId, GmailMessageId, GmailThreadId, GmailWrite, OpKind, OutboxId,
    OutboxState,
};
use crate::gmail::{Backoff, Format, Gmail, GmailError, Outcome, Priority as GmailPriority};
use crate::mime::DRAFT_HEADER;
use crate::store::{
    DraftRecord, NewOp, OpRecord, OutboxRecord, Priority as StorePriority, StoreError, WriteTx,
};

use super::super::actions::{apply_outbox_snapshot, outbox_snapshot, restore_draft_from_outbox};
use super::super::drafts::build_draft_mime;
use super::super::events::Effect;
use super::super::outbox::{OutboxCommand, OutboxEvent, step as step_outbox};
use super::super::writer_state::{DraftOpPayload, DraftOpPayloadRef, TrashSentOpPayload};
use super::drafts::download_carried_draft_attachments;
use super::ops::{OpOutcome, apply_worker_outcome, classify_error, delete_op_now, wait_kick};
use super::queue::FetchPriority;
use super::worker::AccountSlot;

fn ready_outbox_rows(slot: &AccountSlot) -> Vec<OutboxRecord> {
    slot.pool
        .with(|db| {
            let all = db.outbox_unsent(slot.account)?;
            let mut ready = Vec::with_capacity(all.len());
            for row in all {
                let waiting_on_body = db
                    .draft(&row.item.draft)?
                    .is_some_and(|d| d.lifecycle.waits_for.is_some());
                if !waiting_on_body {
                    ready.push(row);
                }
            }
            Ok::<_, StoreError>(ready)
        })
        .unwrap_or_default()
}

pub(crate) async fn run_outbox(slot: Arc<AccountSlot>, cancel: CancellationToken) {
    let mut backoff = Backoff::new();
    let mut wait: Option<Duration> = None;

    loop {
        if cancel.is_cancelled() {
            return;
        }
        let Some(guard) = wait_kick(&slot.outbox_kick, &slot, &mut wait, &cancel).await else {
            return;
        };

        loop {
            if cancel.is_cancelled() {
                drop(guard);
                return;
            }
            let rows = ready_outbox_rows(&slot);
            if rows.is_empty() {
                backoff.reset();
                break;
            }

            let now = slot.clock.now();
            let next_row = rows
                .iter()
                .find(|r| r.item.state == OutboxState::Sending)
                .or_else(|| rows.iter().find(|r| r.item.send_after <= now));

            let Some(row) = next_row else {
                if let Some(earliest) = rows.iter().map(|r| r.item.send_after).min() {
                    let ms = u64::try_from(earliest.0.saturating_sub(now.0)).unwrap_or(0);
                    wait = Some(Duration::from_millis(ms.max(1)));
                }
                break;
            };

            if !slot.is_online() {
                defer_outbox_offline(&slot, row).await;
                break;
            }
            let Some((grant_gen, gmail)) = slot.gmail() else {
                defer_outbox_auth(&slot, row).await;
                break;
            };

            let Some(outcome) = cancel
                .run_until_cancelled(process_outbox_row(&slot, &gmail, row))
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

pub(super) async fn execute_draft_op(
    slot: &AccountSlot,
    gmail: &Gmail,
    op: &OpRecord,
) -> OpOutcome {
    let Ok(payload) = op.parse_payload::<DraftOpPayload>() else {
        delete_op_now(slot, op.id).await;
        return OpOutcome::Done;
    };
    match op.kind {
        OpKind::DraftUpsert => execute_draft_upsert(slot, gmail, op, &payload.draft_id).await,
        OpKind::DraftDelete => {
            execute_draft_delete(
                slot,
                gmail,
                op,
                &payload.draft_id,
                payload.gmail_draft_id.as_ref(),
            )
            .await
        }
        OpKind::Modify | OpKind::CreateLabel | OpKind::TrashMessage => {
            delete_op_now(slot, op.id).await;
            OpOutcome::Done
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "downloads carried attachments, writes draft to Gmail, and reconciles local draft or orphan delete"
)]
async fn execute_draft_upsert(
    slot: &AccountSlot,
    gmail: &Gmail,
    op: &OpRecord,
    draft_id: &DraftId,
) -> OpOutcome {
    if let Err(err) = download_carried_draft_attachments(
        slot.account,
        draft_id,
        gmail,
        &slot.pool,
        &slot.writer,
        &slot.files,
        GmailPriority::P1,
    )
    .await
    {
        return classify_error(slot, op.id, err).await;
    }

    let Some(draft) = slot.pool.with(|db| db.draft(draft_id)).ok().flatten() else {
        delete_op_now(slot, op.id).await;
        return OpOutcome::Done;
    };
    if draft.in_outbox || draft.lifecycle.waits_for.is_some() {
        delete_op_now(slot, op.id).await;
        return OpOutcome::Done;
    }

    match write_draft_to_gmail(slot, gmail, &draft, GmailPriority::P1).await {
        Ok((remote_draft, remote_msg, remote_thread)) => {
            let acct = slot.account;
            let op_id = op.id;
            let local_draft = draft.id.clone();
            let old_gid = draft.lifecycle.gmail_draft_id.clone();
            super::super::log_err(
                "save synced draft ids",
                slot.writer
                    .call(StorePriority::Sync, move |tx, state| {
                        tx.delete_op(op_id, acct)?;
                        match tx.read().draft(&local_draft)? {
                            Some(mut rec) if !rec.in_outbox => {
                                let superseded = rec.lifecycle.gmail_draft_id != old_gid
                                    && rec.lifecycle.gmail_draft_id.as_ref() != Some(&remote_draft);
                                if superseded {
                                    attach_created_id_to_delete(
                                        tx,
                                        acct,
                                        &local_draft,
                                        &remote_draft,
                                    )?;
                                    state.emit(Effect::KickOps(acct));
                                } else {
                                    rec.lifecycle.record_gmail_ids(
                                        remote_draft,
                                        remote_msg,
                                        remote_thread,
                                    );
                                    tx.put_draft(&rec.as_write())?;
                                }
                            }
                            _ => {
                                let owned_by_outbox =
                                    tx.read().outbox_unsent(acct)?.iter().any(|ob| {
                                        ob.item.draft == local_draft
                                            && ob.item.gmail_draft_id.as_ref()
                                                == Some(&remote_draft)
                                    });
                                if !owned_by_outbox {
                                    attach_created_id_to_delete(
                                        tx,
                                        acct,
                                        &local_draft,
                                        &remote_draft,
                                    )?;
                                    state.emit(Effect::KickOps(acct));
                                }
                            }
                        }
                        Ok::<_, StoreError>(())
                    })
                    .await,
            );
            OpOutcome::Done
        }
        Err(WriteDraftError::Gmail(err)) => classify_error(slot, op.id, err).await,
        Err(WriteDraftError::Build(_)) => {
            delete_op_now(slot, op.id).await;
            OpOutcome::Done
        }
    }
}

fn attach_created_id_to_delete(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    draft_id: &DraftId,
    gmail_draft_id: &GmailDraftId,
) -> Result<(), StoreError> {
    let ops = tx.read().all_ops(account)?;
    let mut updated = false;
    for op in ops {
        if op.kind == OpKind::DraftDelete && op.draft_id.as_ref() == Some(draft_id) {
            tx.delete_op(op.id, account)?;
            tx.queue_op(&NewOp {
                account,
                action: None,
                kind: OpKind::DraftDelete,
                draft_id: Some(draft_id),
                gmail_draft_id: Some(gmail_draft_id),
                payload: &DraftOpPayloadRef {
                    draft_id,
                    gmail_draft_id: Some(gmail_draft_id),
                },
            })?;
            updated = true;
            break;
        }
    }
    if !updated {
        tx.queue_op(&NewOp {
            account,
            action: None,
            kind: OpKind::DraftDelete,
            draft_id: Some(draft_id),
            gmail_draft_id: Some(gmail_draft_id),
            payload: &DraftOpPayloadRef {
                draft_id,
                gmail_draft_id: Some(gmail_draft_id),
            },
        })?;
    }
    Ok(())
}

async fn execute_draft_delete(
    slot: &AccountSlot,
    gmail: &Gmail,
    op: &OpRecord,
    draft_id: &DraftId,
    known_gid: Option<&GmailDraftId>,
) -> OpOutcome {
    let target_ids = if let Some(gid) = known_gid.or(op.gmail_draft_id.as_ref()) {
        vec![gid.clone()]
    } else {
        match find_gmail_drafts_by_local_id(gmail, draft_id, GmailPriority::P1).await {
            Ok(ids) => ids,
            Err(err) => return classify_error(slot, op.id, err).await,
        }
    };

    for gid in target_ids {
        match gmail.delete_draft(&gid, GmailPriority::P1).await {
            Ok(()) => {}
            Err(err) if err.outcome == Outcome::NotFound => {}
            Err(err) => return classify_error(slot, op.id, err).await,
        }
    }
    delete_op_now(slot, op.id).await;
    OpOutcome::Done
}

enum WriteDraftError {
    Gmail(GmailError),
    Build(String),
}

async fn write_draft_to_gmail(
    slot: &AccountSlot,
    gmail: &Gmail,
    draft: &DraftRecord,
    priority: GmailPriority,
) -> Result<(GmailDraftId, Option<GmailMessageId>, Option<GmailThreadId>), WriteDraftError> {
    let identity = slot
        .pool
        .with(|db| Ok::<_, StoreError>(db.account(slot.account)?.map(|a| a.identity())))
        .ok()
        .flatten()
        .ok_or_else(|| WriteDraftError::Build("account missing".to_owned()))?;
    let raw =
        build_draft_mime(draft, &identity, slot.clock.now()).map_err(WriteDraftError::Build)?;
    let thread_id = draft.lifecycle.gmail_thread_id.as_ref();

    let resolved = match draft.lifecycle.next_gmail_write() {
        GmailWrite::Update(gid) => Some(gid),
        GmailWrite::Recover => {
            let found = find_gmail_drafts_by_local_id(gmail, &draft.id, priority)
                .await
                .map_err(WriteDraftError::Gmail)?;
            for extra in found.iter().skip(1) {
                super::super::log_err(
                    "delete duplicate recovered draft",
                    gmail.delete_draft(extra, priority).await,
                );
            }
            found.into_iter().next()
        }
        GmailWrite::Create => None,
    };

    if let Some(gid) = resolved {
        match gmail.update_draft(&gid, &raw, thread_id, priority).await {
            Ok(updated) => {
                let remote_msg = updated.message.as_ref().map(|m| m.id.clone());
                let remote_thread = updated.message.and_then(|m| m.thread_id);
                return Ok((updated.id, remote_msg, remote_thread));
            }
            Err(err) if err.outcome == Outcome::NotFound => {}
            Err(err) => return Err(WriteDraftError::Gmail(err)),
        }
    }

    let local_draft = draft.id.clone();
    super::super::log_err(
        "mark gmail_create_sent before drafts.create",
        slot.writer
            .call(StorePriority::Sync, move |tx, _state| {
                if let Some(mut rec) = tx.read().draft(&local_draft)? {
                    rec.lifecycle.mark_create_sent();
                    tx.put_draft(&rec.as_write())?;
                }
                Ok::<_, StoreError>(())
            })
            .await,
    );

    create_or_recover_draft(slot, gmail, draft, &raw, priority).await
}

async fn create_or_recover_draft(
    slot: &AccountSlot,
    gmail: &Gmail,
    draft: &DraftRecord,
    raw: &[u8],
    priority: GmailPriority,
) -> Result<(GmailDraftId, Option<GmailMessageId>, Option<GmailThreadId>), WriteDraftError> {
    let draft_id = &draft.id;
    let thread_id = draft.lifecycle.gmail_thread_id.as_ref();
    match gmail.create_draft(raw, thread_id, priority).await {
        Ok(created) => {
            let remote_msg = created.message.as_ref().map(|m| m.id.clone());
            let remote_thread = created.message.and_then(|m| m.thread_id);
            Ok((created.id, remote_msg, remote_thread))
        }
        Err(err) if err.outcome == Outcome::NotFound && thread_id.is_some() => {
            let local_draft = draft_id.clone();
            super::super::log_err(
                "clear missing thread id from draft",
                slot.writer
                    .call(StorePriority::Sync, move |tx, _state| {
                        if let Some(mut rec) = tx.read().draft(&local_draft)? {
                            rec.lifecycle.gmail_thread_id = None;
                            tx.put_draft(&rec.as_write())?;
                        }
                        Ok::<_, StoreError>(())
                    })
                    .await,
            );
            let created = gmail
                .create_draft(raw, None, priority)
                .await
                .map_err(WriteDraftError::Gmail)?;
            let remote_msg = created.message.as_ref().map(|m| m.id.clone());
            let remote_thread = created.message.and_then(|m| m.thread_id);
            Ok((created.id, remote_msg, remote_thread))
        }
        Err(err) if err.outcome == Outcome::NoResponse => {
            let found = find_gmail_drafts_by_local_id(gmail, draft_id, priority)
                .await
                .map_err(WriteDraftError::Gmail)?;
            for extra in found.iter().skip(1) {
                super::super::log_err(
                    "delete duplicate recovered draft after noResponse",
                    gmail.delete_draft(extra, priority).await,
                );
            }
            if let Some(gid) = found.into_iter().next() {
                let updated = gmail
                    .update_draft(&gid, raw, thread_id, priority)
                    .await
                    .map_err(WriteDraftError::Gmail)?;
                let remote_msg = updated.message.as_ref().map(|m| m.id.clone());
                let remote_thread = updated.message.and_then(|m| m.thread_id);
                Ok((updated.id, remote_msg, remote_thread))
            } else {
                Err(WriteDraftError::Gmail(err))
            }
        }
        Err(err) => Err(WriteDraftError::Gmail(err)),
    }
}

async fn find_gmail_drafts_by_local_id(
    gmail: &Gmail,
    draft_id: &DraftId,
    priority: GmailPriority,
) -> Result<Vec<GmailDraftId>, GmailError> {
    let mut stubs = Vec::new();
    let mut page_token = None;
    loop {
        let page = gmail.list_drafts(page_token.as_ref(), priority).await?;
        stubs.extend(page.drafts);
        if page.next_page_token.is_none() {
            break;
        }
        page_token = page.next_page_token;
    }

    let mut matches = Vec::new();
    for stub in stubs {
        let full = match gmail.get_draft(&stub.id, Format::Metadata, priority).await {
            Ok(d) => d,
            Err(err) if err.outcome == Outcome::NotFound => continue,
            Err(err) => return Err(err),
        };
        let hdr = full
            .message
            .as_ref()
            .and_then(|m| m.header(DRAFT_HEADER))
            .map(str::trim);
        if hdr == Some(draft_id.as_str()) {
            matches.push(full.id);
        }
    }
    Ok(matches)
}

#[expect(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "orchestrates outbox probe, attachment download, Gmail draft upsert, undo check, and send"
)]
async fn process_outbox_row(slot: &AccountSlot, gmail: &Gmail, row: &OutboxRecord) -> OpOutcome {
    let outbox_id = row.item.id;

    if row.item.state == OutboxState::Sending {
        if let Some(gid) = &row.item.gmail_draft_id {
            return probe_uncertain_outbox(slot, gmail, outbox_id, gid).await;
        }
        if row.item.undo_requested {
            apply_outbox_event(slot, outbox_id, OutboxEvent::ProbeDraftPresent).await;
            return OpOutcome::Done;
        }
    } else {
        let proceed = transition_outbox_due(slot, outbox_id).await;
        if !proceed {
            return OpOutcome::Done;
        }
    }

    if let Err(err) = download_carried_draft_attachments(
        slot.account,
        &row.item.draft,
        gmail,
        &slot.pool,
        &slot.writer,
        &slot.files,
        GmailPriority::P1,
    )
    .await
    {
        return handle_outbox_draft_write_error(slot, outbox_id, err).await;
    }

    let Some(draft) = slot
        .pool
        .with(|db| db.draft(&row.item.draft))
        .ok()
        .flatten()
    else {
        apply_outbox_event(
            slot,
            outbox_id,
            OutboxEvent::SendRejected("Draft is missing".to_owned()),
        )
        .await;
        return OpOutcome::Done;
    };

    let gmail_draft_id = match write_draft_to_gmail(slot, gmail, &draft, GmailPriority::P1).await {
        Ok((remote_draft, remote_msg, remote_thread)) => {
            let local_draft = draft.id.clone();
            let draft_id_copy = remote_draft.clone();
            super::super::log_err(
                "persist outbox gmail draft id",
                slot.writer
                    .call(StorePriority::Sync, move |tx, _state| {
                        if let Some(mut rec) = tx.read().draft(&local_draft)? {
                            rec.lifecycle.record_gmail_ids(
                                draft_id_copy.clone(),
                                remote_msg,
                                remote_thread,
                            );
                            tx.put_draft(&rec.as_write())?;
                        }
                        if let Some(mut ob) = tx.read().outbox_row(outbox_id)? {
                            let snap = outbox_snapshot(&ob.item);
                            let (next, _) =
                                step_outbox(snap, OutboxEvent::DraftWritten(draft_id_copy));
                            if let Some(n) = next {
                                apply_outbox_snapshot(&mut ob.item, &n);
                                tx.update_outbox(&ob.item)?;
                            }
                        }
                        Ok::<_, StoreError>(())
                    })
                    .await,
            );
            remote_draft
        }
        Err(WriteDraftError::Gmail(err)) => {
            return handle_outbox_draft_write_error(slot, outbox_id, err).await;
        }
        Err(WriteDraftError::Build(msg)) => {
            apply_outbox_event(
                slot,
                outbox_id,
                OutboxEvent::DraftWriteFailed {
                    transient: false,
                    message: msg,
                },
            )
            .await;
            return OpOutcome::Done;
        }
    };

    let cancelled_before_send = slot
        .pool
        .with(|db| {
            Ok::<_, StoreError>(
                db.outbox_row(outbox_id)?
                    .is_some_and(|r| r.item.undo_requested),
            )
        })
        .unwrap_or(false);
    if cancelled_before_send {
        apply_outbox_event(slot, outbox_id, OutboxEvent::ProbeDraftPresent).await;
        return OpOutcome::Done;
    }

    send_gmail_draft(slot, gmail, outbox_id, &gmail_draft_id).await
}

async fn probe_uncertain_outbox(
    slot: &AccountSlot,
    gmail: &Gmail,
    outbox_id: OutboxId,
    gmail_draft_id: &GmailDraftId,
) -> OpOutcome {
    match gmail
        .get_draft(gmail_draft_id, Format::Minimal, GmailPriority::P1)
        .await
    {
        Ok(_) => {
            let cmd = apply_outbox_event(slot, outbox_id, OutboxEvent::ProbeDraftPresent).await;
            if matches!(cmd, OutboxCommand::RetrySend) {
                send_gmail_draft(slot, gmail, outbox_id, gmail_draft_id).await
            } else {
                OpOutcome::Done
            }
        }
        Err(err) if err.outcome == Outcome::NotFound => {
            apply_outbox_event(slot, outbox_id, OutboxEvent::ProbeDraftMissing).await;
            slot.kick_poll();
            OpOutcome::Done
        }
        Err(err) if matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized) => {
            apply_outbox_event(slot, outbox_id, OutboxEvent::AuthFailed).await;
            OpOutcome::AuthFailed
        }
        Err(err) => OpOutcome::Retry(err),
    }
}

async fn send_gmail_draft(
    slot: &AccountSlot,
    gmail: &Gmail,
    outbox_id: OutboxId,
    gmail_draft_id: &GmailDraftId,
) -> OpOutcome {
    match gmail.send_draft(gmail_draft_id, GmailPriority::P1).await {
        Ok(sent) => {
            let thread_id = sent
                .thread_id
                .unwrap_or_else(|| GmailThreadId(sent.id.0.clone()));
            apply_outbox_event(
                slot,
                outbox_id,
                OutboxEvent::SendSucceeded {
                    message_id: sent.id,
                    thread_id: thread_id.clone(),
                },
            )
            .await;
            slot.fetch_queue.enqueue(
                thread_id,
                Format::Full,
                FetchPriority::SentThread,
                false,
                None,
                &slot.activity,
            );
            OpOutcome::Done
        }
        Err(err) if err.outcome == Outcome::Offline => {
            handle_outbox_draft_write_error(slot, outbox_id, err).await
        }
        Err(err) if err.is_transient() => {
            apply_outbox_event(slot, outbox_id, OutboxEvent::SendUncertain).await;
            OpOutcome::Retry(err)
        }
        Err(err) if matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized) => {
            apply_outbox_event(slot, outbox_id, OutboxEvent::AuthFailed).await;
            OpOutcome::AuthFailed
        }
        Err(err) => {
            apply_outbox_event(slot, outbox_id, OutboxEvent::SendRejected(err.detail)).await;
            OpOutcome::Done
        }
    }
}

async fn handle_outbox_draft_write_error(
    slot: &AccountSlot,
    outbox_id: OutboxId,
    err: GmailError,
) -> OpOutcome {
    if matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized) {
        apply_outbox_event(slot, outbox_id, OutboxEvent::AuthFailed).await;
        return OpOutcome::AuthFailed;
    }
    let transient = err.is_transient();
    let msg = err.detail.clone();
    apply_outbox_event(
        slot,
        outbox_id,
        OutboxEvent::DraftWriteFailed {
            transient,
            message: msg,
        },
    )
    .await;
    if transient {
        OpOutcome::Retry(err)
    } else {
        OpOutcome::Done
    }
}

async fn transition_outbox_due(slot: &AccountSlot, outbox_id: OutboxId) -> bool {
    let cmd = apply_outbox_event(slot, outbox_id, OutboxEvent::Due).await;
    matches!(cmd, OutboxCommand::Persist)
}

async fn defer_outbox_offline(slot: &AccountSlot, row: &OutboxRecord) {
    if row.item.state == OutboxState::Waiting && !row.item.delayed {
        apply_outbox_event(slot, row.item.id, OutboxEvent::DeferredOffline).await;
    }
}

async fn defer_outbox_auth(slot: &AccountSlot, row: &OutboxRecord) {
    apply_outbox_event(slot, row.item.id, OutboxEvent::AuthFailed).await;
}

#[expect(
    clippy::too_many_lines,
    reason = "applies outbox FSM command to SQLite outbox, draft, undo, and notice tables atomically"
)]
async fn apply_outbox_event(
    slot: &AccountSlot,
    outbox_id: OutboxId,
    event: OutboxEvent,
) -> OutboxCommand {
    let acct = slot.account;
    slot.writer
        .call(StorePriority::Sync, move |tx, state| {
            let Some(mut rec) = tx.read().outbox_row(outbox_id)? else {
                return Ok::<_, StoreError>(OutboxCommand::None);
            };
            let snap = outbox_snapshot(&rec.item);
            let (next, cmd) = step_outbox(snap, event);
            match &cmd {
                OutboxCommand::None => {}
                OutboxCommand::Persist | OutboxCommand::RetrySend => {
                    if let Some(n) = next {
                        apply_outbox_snapshot(&mut rec.item, &n);
                        tx.update_outbox(&rec.item)?;
                    }
                }
                OutboxCommand::CancelToDraft { notice } => {
                    restore_draft_from_outbox(tx, outbox_id, notice.clone())?;
                }
                OutboxCommand::TrashSentAndDelete { message_id } => {
                    tx.delete_undo_for_outbox(acct, outbox_id)?;
                    tx.delete_outbox(outbox_id)?;
                    tx.delete_draft(&rec.item.draft)?;
                    state.emit(Effect::RemoveDraftFiles(rec.item.draft));
                    tx.queue_op(&NewOp {
                        account: acct,
                        action: None,
                        kind: OpKind::TrashMessage,
                        draft_id: None,
                        gmail_draft_id: None,
                        payload: &TrashSentOpPayload {
                            gmail_msg_id: message_id.clone(),
                        },
                    })?;
                    state.emit(Effect::KickOps(acct));
                    state.emit(Effect::RefreshBadge);
                }
                OutboxCommand::DeleteRowAndUndo => {
                    tx.delete_undo_for_outbox(acct, outbox_id)?;
                    tx.delete_outbox(outbox_id)?;
                    tx.delete_draft(&rec.item.draft)?;
                    state.emit(Effect::RemoveDraftFiles(rec.item.draft));
                }
                OutboxCommand::FetchSentThread { .. } => {
                    if let Some(n) = next {
                        apply_outbox_snapshot(&mut rec.item, &n);
                        tx.update_outbox(&rec.item)?;
                    } else {
                        tx.delete_undo_for_outbox(acct, outbox_id)?;
                        tx.delete_outbox(outbox_id)?;
                        tx.delete_draft(&rec.item.draft)?;
                        state.emit(Effect::RemoveDraftFiles(rec.item.draft));
                    }
                }
            }
            Ok(cmd)
        })
        .await
        .unwrap_or(OutboxCommand::None)
}
