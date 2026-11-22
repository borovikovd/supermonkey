use sm_domain::{AccountId, DraftId, OutboxId, ThreadId};
use sm_store::{NewNotice, StoreError, WriteTx};

use super::machine::{Event, Step, step};
use super::queue::{finish, trash_sent};
use crate::drafts::lifecycle;
use crate::drafts::queue_upsert;
use crate::writer::WriterState;

pub(crate) fn resolve(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    outbox: OutboxId,
    event: Event,
) -> Result<bool, StoreError> {
    let Some(record) = tx.read().outbox().get(outbox)? else {
        return Ok(false);
    };
    let row = record.outbox;
    let account = row.account;
    let stored = matches!(event, Event::Stored(_));
    let step = step(row, event);
    let applied = step != Step::Ignore;
    match step {
        Step::Ignore => {}
        Step::Write(row) => tx.outbox().update(&row)?,
        Step::Sent {
            row,
            trash,
            drop_entry,
        } => {
            tx.outbox().update(&row)?;
            let copies = tx.read().messages().draft_copies(account, &row.draft)?;
            tx.messages().delete(&copies)?;
            if let Some(message) = &trash {
                trash_sent(tx, state, account, message)?;
            }
            if drop_entry {
                tx.undo().delete_for_outbox(outbox)?;
            }
        }
        Step::Return {
            draft,
            kind,
            message,
        } => {
            tx.undo().delete_for_outbox(outbox)?;
            tx.outbox().delete(outbox)?;
            let thread = keep_draft(tx, state, account, &draft)?;
            tx.notices().add(&NewNotice {
                account,
                kind,
                message,
                draft: Some(draft),
                thread,
            })?;
        }
    }
    if stored {
        finish(tx, state, account)?;
    }
    Ok(applied)
}

pub(super) fn keep_draft(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    account: AccountId,
    draft: &DraftId,
) -> Result<Option<ThreadId>, StoreError> {
    let Some(record) = tx.read().drafts().get(draft)? else {
        return Ok(None);
    };
    let mut kept = record.draft;
    lifecycle::keep(&mut kept);
    tx.drafts().put(&kept)?;
    queue_upsert(tx, state, account, &kept.id)?;
    Ok(kept.thread)
}
