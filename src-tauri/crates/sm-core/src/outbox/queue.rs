use std::time::Duration;

use sm_domain::{
    AccountId, ActionNo, DraftId, GmailMessageId, LabelSet, Millis, OutboxId, OutboxState,
    SystemLabel, UndoEntryId,
};
use sm_store::{NewOp, NewOutbox, OpKind, StoreError, UndoAction, WriteTx};

use super::resolve::keep_draft;
use crate::actions::{ActionError, Unsendable};
use crate::drafts::lifecycle;
use crate::effects::Effect;
use crate::ops::queue_op;
use crate::writer::WriterState;

const SEND_DELAY: Duration = Duration::from_secs(10);

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
    if let Some(bad) = recipients.find(|a| !sm_mime::is_valid_email(&a.email)) {
        return Err(Unsendable::BadRecipient(bad.email.clone()).into());
    }
    tx.ops().delete_draft_upserts(id)?;
    if lifecycle::composer_closed(&mut draft) {
        tx.drafts().put(&draft)?;
    }
    let delay = i64::try_from(SEND_DELAY.as_millis()).unwrap_or(i64::MAX);
    let outbox = tx.outbox().insert(NewOutbox {
        account: draft.account,
        draft: draft.id,
        thread: draft.thread,
        send_after: now.plus(delay),
    })?;
    tx.undo().record(
        draft.account,
        UndoEntryId(n.0),
        &UndoAction::Send { outbox },
    )?;
    state.effect(Effect::Outbox(draft.account));
    Ok(())
}

pub(crate) fn undo_send(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    outbox: OutboxId,
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
            keep_draft(tx, state, account, &row.draft)?;
        }
        OutboxState::Sending => {
            row.undo_requested = true;
            tx.outbox().update(&row)?;
        }
        OutboxState::Sent => {
            if let Some(message) = &row.sent_message_id {
                trash_sent(tx, state, account, message)?;
            }
            tx.outbox().delete(outbox)?;
            tx.drafts().delete(&row.draft)?;
            state.draft_gone(account, row.draft.clone());
            state.effect(Effect::DraftFilesGone(row.draft));
        }
    }
    Ok(())
}

pub(crate) fn trash_sent(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    message: &GmailMessageId,
) -> Result<(), StoreError> {
    if let Some(record) = tx.read().messages().by_gmail_id(account, message)? {
        let gone: LabelSet = [SystemLabel::Inbox, SystemLabel::Spam]
            .into_iter()
            .collect();
        let mut labels = record.labels.without(&gone);
        labels.insert(SystemLabel::Trash.label_id());
        tx.messages().set_labels(&[(record.id, labels)])?;
    }
    queue_op(
        tx,
        state,
        &NewOp {
            account,
            action: None,
            kind: OpKind::TrashMessage,
            draft: None,
            gmail_draft: None,
            payload: message,
        },
    )
}

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

pub(crate) fn finish(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
) -> Result<(), StoreError> {
    for (outbox, draft) in tx.read().outbox().finished(account)? {
        tx.undo().delete_for_outbox(outbox)?;
        tx.outbox().delete(outbox)?;
        tx.drafts().delete(&draft)?;
        state.draft_gone(account, draft.clone());
        state.effect(Effect::DraftFilesGone(draft));
    }
    Ok(())
}

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
