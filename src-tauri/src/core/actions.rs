use std::collections::HashMap;

use crate::domain::query::Query;
use crate::domain::{
    AccountId, ActionNo, DraftId, GmailMessageId, LabelDelta, LabelId, MessageFacts, Millis,
    OpKind, OutboxId, Pin, PinKind, SnoozeEdit, Split, SplitId, SubId, SystemLabel, Thread,
    ThreadChange, ThreadModify, UndoEntryId, plan_modify,
};
use crate::ipc::{
    Action, DraftSave, ModifyAction, RejectReason, Rejection, Setting, SnoozeChange, SplitInfo,
    SubKey, Submitted,
};
use crate::store::{Db, NewNotice, NewOp, NewOutbox, StoreError, UndoAction, WriteTx};

use super::drafts::{
    CreateDraftArgs, apply_update_fields, build_new_draft, carried_sources_needing_download,
    draft_is_untouched, validate_sendable,
};
use super::events::Effect;
use super::outbox::{OutboxCommand, OutboxEvent, OutboxSnapshot, step as step_outbox};
use super::writer_state::{
    CreateLabelOpPayload, DraftOpPayloadRef, ModifyOpPayload, SubOp, TrashSentOpPayload,
    WriterState,
};

pub(crate) const SEND_DELAY_MS: i64 = 10_000;

#[derive(Debug)]
struct ActionReject {
    message: String,
    reason: RejectReason,
}

impl ActionReject {
    fn other(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            reason: RejectReason::Other,
        }
    }

    fn split_rule(split: SplitId, msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            reason: RejectReason::SplitRule { split },
        }
    }
}

enum StepErr {
    Reject(ActionReject),
    Store(StoreError),
}

impl From<StoreError> for StepErr {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

impl From<ActionReject> for StepErr {
    fn from(value: ActionReject) -> Self {
        Self::Reject(value)
    }
}

pub(crate) fn apply_batch(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    batch: Vec<Submitted>,
) -> Result<(), StoreError> {
    let mut last_ack = tx.read().last_action()?;
    for item in &batch {
        if item.n > last_ack {
            state.staged_batch_ns.push(item.n);
        }
    }

    for item in batch {
        if item.n <= last_ack {
            continue;
        }
        last_ack = item.n;
        match item.action {
            Err(bad) => {
                tracing::warn!(n = item.n.0, kind = ?bad.kind, "refused unreadable action");
                state.staged_rejections.push(Rejection {
                    n: item.n,
                    message: "Couldn't do that because of a bug in Supermonkey".to_owned(),
                    reason: RejectReason::Other,
                });
            }
            Ok(action) => {
                let mark = state.effects_mark();
                let res = tx.savepoint(|tx| apply_one(tx, state, item.n, action));
                match res {
                    Ok(()) => {}
                    Err(StepErr::Reject(rej)) => {
                        state.truncate_staged(mark);
                        state.staged_rejections.push(Rejection {
                            n: item.n,
                            message: rej.message,
                            reason: rej.reason,
                        });
                    }
                    Err(StepErr::Store(err)) => return Err(err),
                }
            }
        }
    }

    tx.set_last_action(last_ack)?;
    state.staged_ack = Some(last_ack);
    Ok(())
}

fn apply_one(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    n: ActionNo,
    action: Action,
) -> Result<(), StepErr> {
    match action {
        Action::Reset => apply_reset(tx, state),
        Action::Subscribe { sub, key } => apply_subscribe(tx, state, sub, key),
        Action::Unsubscribe { sub } => {
            state.stage_sub(SubOp::Unsubscribe(sub.clone()));
            state.emit(Effect::Unsubscribe(sub));
            Ok(())
        }
        Action::Modify(m) => apply_modify(tx, state, n, &m),
        Action::Undo { account, entry } => apply_undo(tx, state, account, entry),
        Action::CreateLabel {
            account,
            id,
            name,
            apply,
        } => apply_create_label(tx, state, n, account, id, &name, apply.as_ref()),
        Action::SaveDraft { draft, save } => apply_save_draft(tx, state, draft, save),
        Action::CloseComposer { draft } => apply_close_composer(tx, state, draft),
        Action::SendDraft { draft } => apply_send_draft(tx, state, n, draft),
        Action::DiscardDraft { draft } => apply_discard_draft(tx, state, draft),
        Action::CloseToast { outbox } => apply_close_toast(tx, state, outbox),
        Action::AckNotice { notice } => {
            tx.delete_notice(notice)?;
            Ok(())
        }
        Action::SetSetting(setting) => apply_setting(tx, state, setting),
        Action::Pin {
            account,
            kind,
            value,
            split,
        } => apply_pin(tx, state, n, account, kind, &value, split),
        Action::SaveSplits {
            account,
            splits,
            deleted,
        } => apply_save_splits(tx, account, &splits, &deleted),
        Action::RemoveAccount { account } => apply_remove_account(tx, state, account),
        Action::MoreResults { sub } => {
            state.emit(Effect::MoreResults { sub });
            Ok(())
        }
    }
}

fn require_account(db: Db<'_>, account: AccountId) -> Result<crate::store::AccountRecord, StepErr> {
    db.account(account)?
        .ok_or_else(|| ActionReject::other("That account is no longer in Supermonkey").into())
}

fn apply_reset(tx: &mut WriteTx<'_>, state: &mut WriterState) -> Result<(), StepErr> {
    state.stage_sub(SubOp::Reset);
    state.emit(Effect::ResetSubs);
    let accounts = tx.read().accounts()?;
    for acct in accounts {
        let drafts = tx.read().drafts_for_account(acct.id)?;
        for mut draft in drafts {
            if draft.in_outbox || draft.lifecycle.is_kept() {
                continue;
            }
            if draft_is_untouched(&draft) {
                discard_draft_record(tx, state, draft)?;
            } else {
                draft.lifecycle.keep();
                tx.put_draft(&draft.as_write())?;
            }
        }
    }
    Ok(())
}

fn apply_subscribe(
    tx: &WriteTx<'_>,
    state: &mut WriterState,
    sub: SubId,
    key: SubKey,
) -> Result<(), StepErr> {
    state.stage_sub(SubOp::Subscribe(sub.clone(), key.clone()));
    match &key {
        SubKey::Conversation { thread, .. } => {
            if let Some(row) = tx.read().thread_row(*thread)? {
                state.emit(Effect::OpenConversation {
                    account: row.account,
                    thread: *thread,
                });
            }
        }
        SubKey::List { account, view } if view.gmail_listing().is_some() => {
            state.emit(Effect::SubscribeList {
                sub,
                account: *account,
                key,
            });
        }
        SubKey::Search { account, .. } => {
            state.emit(Effect::SubscribeList {
                sub,
                account: *account,
                key,
            });
        }
        SubKey::Composer { draft } => {
            if let Some(rec) = tx.read().draft(draft)? {
                if rec.lifecycle.waits_for.is_some()
                    && let Some(thread) = rec.thread
                {
                    state.emit(Effect::OpenConversation {
                        account: rec.account,
                        thread,
                    });
                }
                if !carried_sources_needing_download(&rec).is_empty() {
                    state.emit(Effect::EnsureDraftAttachments {
                        account: rec.account,
                        draft: rec.id,
                    });
                }
            }
        }
        SubKey::App | SubKey::Account { .. } | SubKey::List { .. } => {}
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "modify plans per-thread label and snooze deltas, queues ops, and records undo atomically"
)]
fn apply_modify(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    n: ActionNo,
    m: &ModifyAction,
) -> Result<(), StepErr> {
    let _acct = require_account(tx.read(), m.account)?;
    let delta = LabelDelta::new(m.add.clone(), m.remove.clone())
        .map_err(|e| ActionReject::other(e.to_string()))?;
    let snooze_edit = m.snooze.as_ref().map(|s| match s {
        SnoozeChange::Set(at) => SnoozeEdit::Set(*at),
        SnoozeChange::Clear => SnoozeEdit::Clear,
    });
    let mut changes = Vec::new();
    let mut any_op = false;

    for &tid in &m.threads {
        let Some(row) = tx.read().thread_row(tid)? else {
            continue;
        };
        if row.account != m.account {
            continue;
        }
        let msgs = tx.read().thread_messages(tid)?;
        let thread = Thread::new(msgs.iter().map(to_message_facts).collect());
        let change = plan_modify(
            ThreadModify {
                thread_id: tid,
                thread: &thread,
                kind: m.kind,
                snooze_before: row.snooze_until,
                snooze_edit,
            },
            &delta,
        );
        if change.is_noop() {
            continue;
        }

        let by_id: HashMap<_, _> = msgs.iter().map(|msg| (msg.id, msg)).collect();
        let mut changed_ids = Vec::with_capacity(change.messages.len());
        for mc in &change.messages {
            if let Some(msg) = by_id.get(&mc.message) {
                let labels = mc.apply(&msg.labels);
                tx.set_message_labels(msg.id, &labels)?;
                changed_ids.push(msg.gmail_id.clone());
            }
        }
        if change.snooze_after != change.snooze_before {
            tx.set_thread_snooze(tid, change.snooze_after)?;
        }
        tx.derive_thread(tid)?;

        let gmail_msg_ids = if changed_ids.is_empty() {
            Vec::new()
        } else if delta.adds_unread_only() {
            changed_ids
        } else {
            let includes_trashed = delta.remove().iter().any(|l| {
                l.as_str() == SystemLabel::Trash.id() || l.as_str() == SystemLabel::Spam.id()
            });
            let scope_ids: Vec<GmailMessageId> = msgs
                .iter()
                .filter(|msg| !msg.is_draft && (includes_trashed || msg.is_live))
                .map(|msg| msg.gmail_id.clone())
                .collect();
            if scope_ids.is_empty() {
                changed_ids
            } else {
                scope_ids
            }
        };

        if !gmail_msg_ids.is_empty() {
            let payload = ModifyOpPayload {
                thread: tid,
                gmail_msg_ids,
                add: delta.add().to_vec(),
                remove: delta.remove().to_vec(),
                revert: change.clone(),
            };
            tx.queue_op(&NewOp {
                account: m.account,
                action: Some(n),
                kind: OpKind::Modify,
                draft_id: None,
                gmail_draft_id: None,
                payload: &payload,
            })?;
            any_op = true;
        }
        changes.push(change);
    }

    if m.undoable && !changes.is_empty() {
        tx.record_undo(
            UndoEntryId(n.0),
            m.account,
            None,
            &UndoAction::Mail { threads: changes },
        )?;
    }
    if any_op {
        state.emit(Effect::KickOps(m.account));
    }
    state.emit(Effect::KickSnooze);
    state.emit(Effect::RefreshBadge);
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "create label command unpacks action fields with writer transaction and state"
)]
fn apply_create_label(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    n: ActionNo,
    account: AccountId,
    id: LabelId,
    name: &str,
    apply: Option<&ModifyAction>,
) -> Result<(), StepErr> {
    let _acct = require_account(tx.read(), account)?;
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ActionReject::other("Label name cannot be empty").into());
    }
    tx.create_local_label(account, &id, trimmed)?;
    state.freshness.bump_labels(account);
    tx.queue_op(&NewOp {
        account,
        action: Some(n),
        kind: OpKind::CreateLabel,
        draft_id: None,
        gmail_draft_id: None,
        payload: &CreateLabelOpPayload {
            local_id: id,
            name: trimmed.to_owned(),
        },
    })?;
    state.emit(Effect::KickOps(account));
    if let Some(modify) = apply {
        apply_modify(tx, state, n, modify)?;
    }
    Ok(())
}

fn apply_undo(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    entry: UndoEntryId,
) -> Result<(), StepErr> {
    let _acct = require_account(tx.read(), account)?;
    let Some(rec) = tx.read().undo_entry(account, entry)? else {
        return Err(ActionReject::other("That change can no longer be undone").into());
    };
    tx.delete_undo(account, entry)?;
    match rec.action {
        UndoAction::Mail { threads } => undo_modify(tx, state, account, &threads),
        UndoAction::Pin {
            pin_kind,
            value,
            previous,
        } => {
            if let Some(split) = previous {
                tx.set_pin(
                    account,
                    &Pin {
                        kind: pin_kind,
                        value,
                        split,
                    },
                )?;
            } else {
                tx.remove_pin(account, pin_kind, &value)?;
            }
            Ok(())
        }
        UndoAction::Send { outbox } => undo_send(tx, state, account, outbox),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "undo modify reverts label changes grouped by delta and restores prior snooze timestamps"
)]
fn undo_modify(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    changes: &[ThreadChange],
) -> Result<(), StepErr> {
    let mut any_op = false;
    for change in changes {
        let Some(row) = tx.read().thread_row(change.thread)? else {
            continue;
        };
        if row.account != account {
            continue;
        }
        let msgs = tx.read().thread_messages(change.thread)?;
        let by_id: HashMap<_, _> = msgs.iter().map(|m| (m.id, m)).collect();
        let mut by_delta: HashMap<(Vec<LabelId>, Vec<LabelId>), Vec<GmailMessageId>> =
            HashMap::new();
        let mut revert_msgs = Vec::new();

        for mc in &change.messages {
            let Some(msg) = by_id.get(&mc.message) else {
                continue;
            };
            let Some(rev_mc) = mc.reverted(&msg.labels) else {
                continue;
            };
            let labels = rev_mc.apply(&msg.labels);
            tx.set_message_labels(msg.id, &labels)?;
            let mut add = rev_mc.added.clone();
            let mut remove = rev_mc.removed.clone();
            add.sort_by(|a, b| a.0.cmp(&b.0));
            remove.sort_by(|a, b| a.0.cmp(&b.0));
            by_delta
                .entry((add, remove))
                .or_default()
                .push(msg.gmail_id.clone());
            revert_msgs.push(rev_mc);
        }
        let prev_before_undo = row.snooze_until;
        let snooze_now = change.snooze_undone(prev_before_undo);
        tx.set_thread_snooze(change.thread, snooze_now)?;
        tx.derive_thread(change.thread)?;

        for ((add, remove), gmail_msg_ids) in by_delta {
            let payload = ModifyOpPayload {
                thread: change.thread,
                gmail_msg_ids,
                add,
                remove,
                revert: ThreadChange {
                    thread: change.thread,
                    messages: revert_msgs.clone(),
                    snooze_before: prev_before_undo,
                    snooze_after: snooze_now,
                },
            };
            tx.queue_op(&NewOp {
                account,
                action: None,
                kind: OpKind::Modify,
                draft_id: None,
                gmail_draft_id: None,
                payload: &payload,
            })?;
            any_op = true;
        }
    }
    if any_op {
        state.emit(Effect::KickOps(account));
    }
    state.emit(Effect::KickSnooze);
    state.emit(Effect::RefreshBadge);
    Ok(())
}

fn undo_send(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    outbox: OutboxId,
) -> Result<(), StepErr> {
    let Some(mut rec) = tx.read().outbox_row(outbox)? else {
        return Err(ActionReject::other("That change can no longer be undone").into());
    };
    let snap = outbox_snapshot(&rec.item);
    let (next, cmd) = step_outbox(snap, OutboxEvent::UndoRequested);
    match cmd {
        OutboxCommand::CancelToDraft { .. } => {
            tx.delete_outbox(outbox)?;
            keep_cancelled_send_as_draft(tx, state, &rec.item)?;
        }
        OutboxCommand::Persist => {
            if let Some(next_snap) = next {
                apply_outbox_snapshot(&mut rec.item, &next_snap);
                tx.update_outbox(&rec.item)?;
                state.emit(Effect::KickOutbox(account));
            }
        }
        OutboxCommand::TrashSentAndDelete { message_id } => {
            tx.delete_outbox(outbox)?;
            tx.delete_draft(&rec.item.draft)?;
            state.emit(Effect::RemoveDraftFiles(rec.item.draft.clone()));
            trash_local_sent_message(tx, account, &message_id)?;
            tx.queue_op(&NewOp {
                account,
                action: None,
                kind: OpKind::TrashMessage,
                draft_id: None,
                gmail_draft_id: None,
                payload: &TrashSentOpPayload {
                    gmail_msg_id: message_id,
                },
            })?;
            state.emit(Effect::KickOps(account));
            state.emit(Effect::RefreshBadge);
        }
        OutboxCommand::DeleteRowAndUndo => {
            tx.delete_outbox(outbox)?;
            tx.delete_draft(&rec.item.draft)?;
            state.emit(Effect::RemoveDraftFiles(rec.item.draft));
        }
        OutboxCommand::None | OutboxCommand::FetchSentThread { .. } | OutboxCommand::RetrySend => {}
    }
    Ok(())
}

fn trash_local_sent_message(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    gmail_msg_id: &GmailMessageId,
) -> Result<(), StoreError> {
    let Some(msg) = tx.read().message_by_gmail(account, gmail_msg_id)? else {
        return Ok(());
    };
    let labels = msg
        .labels
        .without(&[SystemLabel::Inbox.label_id()])
        .and(&[SystemLabel::Trash.label_id()]);
    tx.set_message_labels(msg.id, &labels)?;
    tx.derive_thread(msg.thread)?;
    Ok(())
}

fn apply_save_draft(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    draft: DraftId,
    save: DraftSave,
) -> Result<(), StepErr> {
    match save {
        DraftSave::Create {
            account,
            kind,
            reply_all,
            thread,
            target,
            started_on_latest,
        } => {
            let _acct = require_account(tx.read(), account)?;
            if tx.read().draft(&draft)?.is_some() {
                return Ok(());
            }
            let new_draft = build_new_draft(
                tx.read(),
                CreateDraftArgs {
                    id: draft,
                    account,
                    kind,
                    reply_all,
                    thread,
                    target,
                    started_on_latest,
                    now: state.clock.now(),
                    offset: state.clock.local_offset(),
                },
            )?;
            let needs_body = new_draft.lifecycle.waits_for.is_some();
            let tid = new_draft.thread;
            tx.put_draft(&new_draft.as_write())?;
            if needs_body && let Some(thread) = tid {
                state.emit(Effect::OpenConversation { account, thread });
            }
        }
        DraftSave::Update { fields, rev } => {
            let Some(mut rec) = tx.read().draft(&draft)? else {
                return Ok(());
            };
            if rec.in_outbox {
                return Ok(());
            }
            apply_update_fields(&mut rec, fields, rev, state.clock.now());
            tx.put_draft(&rec.as_write())?;
            let untouched = draft_is_untouched(&rec);
            if (!untouched || rec.lifecycle.is_kept()) && rec.lifecycle.waits_for.is_none() {
                schedule_draft_upsert(tx, state, &rec)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn schedule_draft_upsert(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    draft: &crate::store::DraftRecord,
) -> Result<(), StoreError> {
    if !carried_sources_needing_download(draft).is_empty() {
        state.emit(Effect::EnsureDraftAttachments {
            account: draft.account,
            draft: draft.id.clone(),
        });
    }
    tx.delete_draft_upserts(draft.account, &draft.id)?;
    tx.queue_op(&NewOp {
        account: draft.account,
        action: None,
        kind: OpKind::DraftUpsert,
        draft_id: Some(&draft.id),
        gmail_draft_id: draft.lifecycle.gmail_draft_id.as_ref(),
        payload: &DraftOpPayloadRef {
            draft_id: &draft.id,
            gmail_draft_id: draft.lifecycle.gmail_draft_id.as_ref(),
        },
    })?;
    state.emit(Effect::KickOps(draft.account));
    Ok(())
}

fn apply_close_composer(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    draft: DraftId,
) -> Result<(), StepErr> {
    let rec = tx.read().draft(&draft)?;
    state.stage_sub(SubOp::DropComposer(draft));
    let Some(mut rec) = rec else {
        return Ok(());
    };
    if rec.in_outbox {
        return Ok(());
    }
    if draft_is_untouched(&rec) {
        discard_draft_record(tx, state, rec)?;
        return Ok(());
    }
    let reload_remote = rec
        .lifecycle
        .take_gmail_change_skipped()
        .then(|| rec.lifecycle.gmail_draft_id.clone())
        .flatten();
    rec.lifecycle.keep();
    tx.put_draft(&rec.as_write())?;
    if let Some(gid) = reload_remote {
        state.emit(Effect::ReloadDraftFromGmail {
            account: rec.account,
            draft: rec.id,
            gmail_draft_id: gid,
        });
    }
    Ok(())
}

fn apply_send_draft(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    n: ActionNo,
    draft: DraftId,
) -> Result<(), StepErr> {
    let Some(mut rec) = tx.read().draft(&draft)? else {
        return Err(ActionReject::other("That draft is gone").into());
    };
    if rec.in_outbox {
        return Err(ActionReject::other("That draft is already sending").into());
    }
    if let Err(msg) = validate_sendable(&rec) {
        return Err(ActionReject::other(msg).into());
    }
    state.stage_sub(SubOp::DropComposer(draft));
    rec.lifecycle.keep();
    rec.lifecycle.gmail_change_skipped = false;
    tx.put_draft(&rec.as_write())?;
    tx.delete_draft_upserts(rec.account, &rec.id)?;
    let send_after = Millis(state.clock.now().0.saturating_add(SEND_DELAY_MS));
    let outbox_id = tx.insert_outbox(&NewOutbox {
        account: rec.account,
        draft: &rec.id,
        thread: rec.thread,
        send_after,
        gmail_draft_id: None,
    })?;
    tx.record_undo(
        UndoEntryId(n.0),
        rec.account,
        Some(outbox_id),
        &UndoAction::Send { outbox: outbox_id },
    )?;
    state.emit(Effect::KickOutbox(rec.account));
    Ok(())
}

fn apply_discard_draft(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    draft: DraftId,
) -> Result<(), StepErr> {
    let rec = tx.read().draft(&draft)?;
    state.stage_sub(SubOp::DropComposer(draft));
    let Some(rec) = rec else {
        return Ok(());
    };
    if rec.in_outbox {
        return Ok(());
    }
    discard_draft_record(tx, state, rec)?;
    Ok(())
}

fn discard_draft_record(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    rec: crate::store::DraftRecord,
) -> Result<(), StoreError> {
    tx.delete_draft(&rec.id)?;
    tx.delete_draft_upserts(rec.account, &rec.id)?;
    if rec.lifecycle.gmail_draft_id.is_some() || rec.lifecycle.gmail_create_sent {
        tx.queue_op(&NewOp {
            account: rec.account,
            action: None,
            kind: OpKind::DraftDelete,
            draft_id: Some(&rec.id),
            gmail_draft_id: rec.lifecycle.gmail_draft_id.as_ref(),
            payload: &DraftOpPayloadRef {
                draft_id: &rec.id,
                gmail_draft_id: rec.lifecycle.gmail_draft_id.as_ref(),
            },
        })?;
        state.emit(Effect::KickOps(rec.account));
    }
    state.emit(Effect::RemoveDraftFiles(rec.id));
    Ok(())
}

fn apply_close_toast(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    outbox: OutboxId,
) -> Result<(), StepErr> {
    let Some(mut rec) = tx.read().outbox_row(outbox)? else {
        return Ok(());
    };
    let snap = outbox_snapshot(&rec.item);
    let (next, cmd) = step_outbox(snap, OutboxEvent::ToastClosed);
    match cmd {
        OutboxCommand::DeleteRowAndUndo => {
            tx.delete_undo_for_outbox(rec.item.account, outbox)?;
            let awaiting_thread_msg = rec.item.thread.is_some()
                && rec.item.sent_message_id.as_ref().is_some_and(|gmid| {
                    tx.read()
                        .message_by_gmail(rec.item.account, gmid)
                        .ok()
                        .flatten()
                        .is_none()
                });
            if awaiting_thread_msg {
                rec.item.toast_closed = true;
                tx.update_outbox(&rec.item)?;
            } else {
                tx.delete_outbox(outbox)?;
                tx.delete_draft(&rec.item.draft)?;
                state.emit(Effect::RemoveDraftFiles(rec.item.draft));
            }
        }
        OutboxCommand::Persist => {
            if let Some(next_snap) = next {
                apply_outbox_snapshot(&mut rec.item, &next_snap);
                tx.update_outbox(&rec.item)?;
            }
        }
        OutboxCommand::None
        | OutboxCommand::CancelToDraft { .. }
        | OutboxCommand::TrashSentAndDelete { .. }
        | OutboxCommand::FetchSentThread { .. }
        | OutboxCommand::RetrySend => {}
    }
    Ok(())
}

fn apply_setting(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    setting: Setting,
) -> Result<(), StepErr> {
    match setting {
        Setting::RemoteImages { on } => {
            tx.set_setting(None, crate::domain::SettingKey::RemoteImages, on)?;
        }
        Setting::Muted { account, on } => {
            let _acct = require_account(tx.read(), account)?;
            tx.set_setting(Some(account), crate::domain::SettingKey::Muted, on)?;
            state.emit(Effect::RefreshBadge);
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "pin command unpacks action fields with writer transaction and state"
)]
fn apply_pin(
    tx: &mut WriteTx<'_>,
    _state: &mut WriterState,
    n: ActionNo,
    account: AccountId,
    kind: PinKind,
    value: &str,
    split: SplitId,
) -> Result<(), StepErr> {
    let _acct = require_account(tx.read(), account)?;
    let trimmed = value.trim();
    let normalized = match kind {
        PinKind::Sender | PinKind::Domain => trimmed.to_ascii_lowercase(),
        PinKind::Label => trimmed.to_owned(),
    };
    let prev = tx.set_pin(
        account,
        &Pin {
            kind,
            value: normalized.clone(),
            split,
        },
    )?;
    tx.record_undo(
        UndoEntryId(n.0),
        account,
        None,
        &UndoAction::Pin {
            pin_kind: kind,
            value: normalized,
            previous: prev,
        },
    )?;
    Ok(())
}

fn apply_save_splits(
    tx: &mut WriteTx<'_>,
    account: AccountId,
    splits: &[SplitInfo],
    deleted: &[SplitId],
) -> Result<(), StepErr> {
    let _acct = require_account(tx.read(), account)?;
    let mut converted = Vec::with_capacity(splits.len());
    for (idx, s) in splits.iter().enumerate() {
        if s.kind == crate::domain::SplitKind::Custom
            && !s.query.trim().is_empty()
            && let Err(err) = Query::parse_rule(&s.query)
        {
            return Err(ActionReject::split_rule(s.id, err.to_string()).into());
        }
        converted.push(Split {
            id: s.id,
            name: s.name.clone(),
            kind: s.kind,
            position: i32::try_from(idx).unwrap_or(i32::MAX),
            query: s.query.clone(),
            enabled: s.enabled,
            hide_when_empty: s.hide_when_empty,
        });
    }
    tx.save_splits(account, &converted, deleted)?;
    Ok(())
}

fn apply_remove_account(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
) -> Result<(), StepErr> {
    let Some(acct) = tx.read().account(account)? else {
        return Ok(());
    };
    let drafts = tx.read().drafts_for_account(account)?;
    let outbox_rows = tx.read().outbox_for_account(account)?;
    for d in drafts {
        state.stage_sub(SubOp::DropComposer(d.id.clone()));
        state.emit(Effect::RemoveDraftFiles(d.id));
    }
    for o in outbox_rows {
        state.emit(Effect::RemoveDraftFiles(o.item.draft));
    }
    state.stage_sub(SubOp::StopSearching(account));
    state.freshness.clear_account(account);
    tx.remove_account(account)?;
    state.emit(Effect::RemoveAccount {
        account,
        email: acct.email,
    });
    state.emit(Effect::RefreshBadge);
    state.emit(Effect::KickSnooze);
    Ok(())
}

fn keep_cancelled_send_as_draft(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    item: &crate::store::OutboxItem,
) -> Result<(), StoreError> {
    let Some(mut draft) = tx.read().draft(&item.draft)? else {
        return Ok(());
    };
    draft.lifecycle.keep();
    tx.put_draft(&draft.as_write())?;
    if item.gmail_draft_id.is_none() {
        schedule_draft_upsert(tx, state, &draft)?;
    }
    Ok(())
}

pub(crate) fn restore_draft_from_outbox(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    outbox: OutboxId,
    notice: Option<super::outbox::OutboxNotice>,
) -> Result<(), StoreError> {
    let Some(rec) = tx.read().outbox_row(outbox)? else {
        return Ok(());
    };
    tx.delete_undo_for_outbox(rec.item.account, outbox)?;
    tx.delete_outbox(outbox)?;
    keep_cancelled_send_as_draft(tx, state, &rec.item)?;
    if let Some(n) = notice {
        let (kind, msg) = match n {
            super::outbox::OutboxNotice::Undone => {
                (crate::domain::NoticeKind::SendUndone, String::new())
            }
            super::outbox::OutboxNotice::Failed(m) => (crate::domain::NoticeKind::SendFailed, m),
        };
        tx.add_notice(&NewNotice {
            account: rec.item.account,
            kind,
            message: &msg,
            draft: Some(&rec.item.draft),
            thread: rec.item.thread,
        })?;
    }
    Ok(())
}

#[must_use]
pub(crate) fn outbox_snapshot(item: &crate::store::OutboxItem) -> OutboxSnapshot {
    OutboxSnapshot {
        state: item.state,
        undo_requested: item.undo_requested,
        toast_closed: item.toast_closed,
        delayed: item.delayed,
        gmail_draft_id: item.gmail_draft_id.clone(),
        sent_message_id: item.sent_message_id.clone(),
    }
}

pub(crate) fn apply_outbox_snapshot(item: &mut crate::store::OutboxItem, snap: &OutboxSnapshot) {
    item.state = snap.state;
    item.undo_requested = snap.undo_requested;
    item.toast_closed = snap.toast_closed;
    item.delayed = snap.delayed;
    item.gmail_draft_id.clone_from(&snap.gmail_draft_id);
    item.sent_message_id.clone_from(&snap.sent_message_id);
}

#[must_use]
fn to_message_facts(rec: &crate::store::MessageRecord) -> MessageFacts {
    MessageFacts {
        id: rec.id,
        gmail_id: rec.gmail_id.clone(),
        date: rec.date,
        from: rec.from.clone(),
        to: rec.to.clone(),
        cc: rec.cc.clone(),
        bcc: rec.bcc.clone(),
        reply_to: rec.reply_to.clone(),
        subject: rec.subject.clone(),
        snippet: rec.snippet.clone(),
        message_id_hdr: rec.message_id_hdr.clone(),
        in_reply_to: rec.in_reply_to.clone(),
        references: rec.references.clone(),
        is_list: rec.is_list,
        is_draft: rec.is_draft,
        has_attachments: rec.has_attachments,
        labels: rec.labels.clone(),
    }
}
