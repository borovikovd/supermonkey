//! Writing a step of a send's state machine.

use sm_domain::{Millis, OutboxId};
use sm_store::{NewNotice, StoreError, WriteTx};

use super::machine::{Event, Step, step};
use super::queue::{finish, trash_sent};
use crate::drafts::lifecycle;
use crate::drafts::queue_upsert;
use crate::writer::WriterState;

/// Applies what the outbox runner saw (`event`) to send `outbox`, and
/// returns whether it applied. A row gone meanwhile (undone, or its account
/// removed) changes nothing.
pub(crate) fn resolve(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    outbox: OutboxId,
    event: Event,
    now: Millis,
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
            // Gmail removed its draft as it sent.
            let copies = tx.read().messages().draft_copies(account, &row.draft)?;
            tx.messages().delete(&copies)?;
            if let Some(message) = &trash {
                trash_sent(tx, state, account, message, now)?;
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
            let thread = match tx.read().drafts().get(&draft)? {
                Some(record) => {
                    let mut kept = record.draft;
                    lifecycle::keep(&mut kept);
                    tx.drafts().put(&kept)?;
                    // Gmail may never have got it: a send made before any
                    // autosave, cut short before its write.
                    queue_upsert(tx, state, account, &kept.id, now)?;
                    kept.thread
                }
                None => None,
            };
            tx.notices().add(&NewNotice {
                account,
                kind,
                message,
                draft: Some(draft),
                thread,
                created_at: now,
            })?;
        }
    }
    if stored {
        finish(tx, state, account)?;
    }
    Ok(applied)
}
