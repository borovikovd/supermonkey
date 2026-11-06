//! Sends in their undo window: queueing, undoing, the toast, and launch.

use std::time::Duration;

use ub_domain::{
    AccountId, ActionNo, DraftId, GmailMessageId, LabelSet, Millis, OutboxId, OutboxState,
    SystemLabel, UndoEntryId,
};
use ub_store::{NewOp, NewOutbox, OpKind, StoreError, UndoAction, WriteTx};

use crate::actions::{ActionError, Unsendable};
use crate::drafts::queue_upsert;
use crate::effects::Effect;
use crate::writer::WriterState;

/// The undo window: a fixed ten seconds, as in Superhuman, which does not
/// let users change it.
const SEND_DELAY: Duration = Duration::from_secs(10);

/// The undo entry's label.
const SENT: &str = "Sent";

/// `sendDraft`: the draft goes into the outbox, waiting out its undo window,
/// as undo entry `n`. Its queued upserts go: the outbox writes the final
/// version itself. A draft with no recipient, or one that is not an
/// address, is refused and stays a draft.
pub(crate) fn send(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    id: &DraftId,
    n: ActionNo,
    now: Millis,
) -> Result<(), ActionError> {
    let Some(record) = tx.read().drafts().get(id)? else {
        return Err(ActionError::Invalid("That draft no longer exists"));
    };
    if record.in_outbox {
        return Err(ActionError::Invalid("That draft is already being sent"));
    }
    let mut draft = record.draft;
    let mut recipients = draft
        .to
        .iter()
        .chain(&draft.cc)
        .chain(&draft.bcc)
        .peekable();
    if recipients.peek().is_none() {
        return Err(Unsendable::NoRecipient.into());
    }
    if let Some(bad) = recipients.find(|a| !ub_mime::is_valid_email(&a.email)) {
        return Err(Unsendable::BadRecipient(bad.email.clone()).into());
    }
    tx.ops().delete_draft_upserts(id)?;
    if draft.started_fresh {
        draft.started_fresh = false;
        tx.drafts().put(&draft)?;
    }
    let delay = i64::try_from(SEND_DELAY.as_millis()).unwrap_or(i64::MAX);
    let outbox = tx.outbox().insert(&NewOutbox {
        account: draft.account,
        draft: id.clone(),
        thread: draft.thread,
        send_after: Millis(now.0.saturating_add(delay)),
    })?;
    tx.undo().record(
        draft.account,
        UndoEntryId(n.0),
        SENT,
        &UndoAction::Send { outbox },
    )?;
    state.effect(Effect::Outbox(draft.account));
    Ok(())
}

/// Undoes a send (SPEC Send and undo). Waiting: it is cancelled and its
/// draft is a draft again, never discarded as untouched and kept on Gmail.
/// Sending: the undo is held until the send resolves. Sent: only that sent
/// message is trashed.
pub(crate) fn undo_send(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    outbox: OutboxId,
    now: Millis,
) -> Result<(), ActionError> {
    let Some(record) = tx.read().outbox().get(outbox)? else {
        return Err(ActionError::Invalid("That send can no longer be undone"));
    };
    let mut row = record.outbox;
    if row.account != account {
        return Err(ActionError::Invalid("That send can no longer be undone"));
    }
    match row.state {
        OutboxState::Waiting => {
            tx.outbox().delete(outbox)?;
            if let Some(draft) = tx.read().drafts().get(&row.draft)? {
                let mut draft = draft.draft;
                draft.started_fresh = false;
                draft.defaults = None;
                tx.drafts().put(&draft)?;
                // A send made before any autosave never wrote it to Gmail.
                queue_upsert(tx, state, account, &draft.id, now)?;
            }
        }
        OutboxState::Sending => {
            row.undo_requested = true;
            tx.outbox().update(&row)?;
        }
        OutboxState::Sent => {
            if let Some(message) = &row.sent_message_id {
                trash_sent(tx, state, account, message, now)?;
            }
            tx.outbox().delete(outbox)?;
            tx.drafts().delete(&row.draft)?;
            state.effect(Effect::DraftFilesGone(row.draft));
        }
    }
    Ok(())
}

/// Trashes only this sent message: here at once, and on Gmail through the
/// queue. Other messages keep their labels.
pub(crate) fn trash_sent(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    message: &GmailMessageId,
    now: Millis,
) -> Result<(), StoreError> {
    if let Some(record) = tx.read().messages().by_gmail_id(account, message)? {
        let gone: LabelSet = [SystemLabel::Inbox, SystemLabel::Spam]
            .into_iter()
            .collect();
        let mut labels = record.labels.without(&gone);
        labels.insert(SystemLabel::Trash.label_id());
        tx.messages().set_labels(&[(record.id, labels)])?;
    }
    tx.ops().push(&NewOp {
        account,
        action: None,
        kind: OpKind::TrashMessage,
        draft: None,
        gmail_draft: None,
        payload: message,
        created_at: now,
    })?;
    state.effect(Effect::Ops(account));
    Ok(())
}

/// The user closed a send toast. A sent row's undo entry goes with it, so a
/// stray `z` never trashes delivered mail. A row already gone is not an
/// error.
pub(crate) fn close_toast(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    outbox: OutboxId,
) -> Result<(), StoreError> {
    let Some(record) = tx.read().outbox().get(outbox)? else {
        return Ok(());
    };
    let mut row = record.outbox;
    if !row.toast_closed {
        row.toast_closed = true;
        tx.outbox().update(&row)?;
    }
    if row.state == OutboxState::Sent {
        tx.undo().delete_for_outbox(outbox)?;
    }
    finish(tx, state, row.account)
}

/// Drops the account's finished sends: sent, the toast closed, and the
/// message on its thread. Their drafts go, and their files after the commit.
pub(crate) fn finish(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
) -> Result<(), StoreError> {
    for (outbox, draft) in tx.read().outbox().finished(account)? {
        tx.undo().delete_for_outbox(outbox)?;
        tx.outbox().delete(outbox)?;
        tx.drafts().delete(&draft)?;
        state.effect(Effect::DraftFilesGone(draft));
    }
    Ok(())
}

/// On launch: sends already sent lose their undo entries and their toasts,
/// so a stray `z` never trashes delivered mail; sends that missed their
/// time show `Email delayed`; finished ones go.
pub(crate) fn on_launch(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    now: Millis,
) -> Result<(), StoreError> {
    for entry in tx.read().undo().for_sent_rows()? {
        tx.undo().delete(entry)?;
    }
    for account in tx.read().accounts().all()? {
        for record in tx.read().outbox().for_account(account.id)? {
            let mut row = record.outbox;
            let sent = row.state == OutboxState::Sent;
            let late = !sent && row.send_after < now;
            if (sent && !row.toast_closed) || (late && !row.delayed) {
                row.toast_closed |= sent;
                row.delayed |= late;
                tx.outbox().update(&row)?;
            }
        }
        finish(tx, state, account.id)?;
    }
    Ok(())
}
