//! One run of one op: its Gmail calls, then its result written.

use std::sync::Arc;

use futures_util::future::join_all;
use sm_domain::{DraftId, GmailMessageId, LabelId, LabelSet};
use sm_gmail::{Format, GmailError, Outcome, Priority};
use sm_store::{OpId, OpKind, OpRecord, Priority as WritePriority, StoreError};

use crate::Error;
use crate::drafts::lifecycle::GmailIds;
use crate::drafts::{WritingDraft, delete_gmail_draft, store_ids};
use crate::effects::Effect;
use crate::ops::{LabelCreateOp, ModifyOp, reject, relabel};
use crate::workers::account::AccountCtx;
use crate::workers::drafts::{Upserted, WriteFailure, delete_copies, delete_draft, upsert};

/// The most ids one `messages.batchModify` takes.
const BATCH_MODIFY_MAX: usize = 1000;

/// Why a run left its op queued.
#[derive(Debug)]
pub(super) enum Failure {
    /// Gmail did not answer clearly: offline, a transient error, a refused
    /// grant.
    Gmail(GmailError),
    /// The database refused a read or write.
    Store(Error),
    /// The outbox is writing the op's draft: run it again once it is done.
    Busy,
}

/// What Gmail said, as the op's result to write.
#[derive(Debug)]
enum Answer {
    /// Done: the op leaves the queue.
    Done(Landing),
    /// Some of its messages are gone: run it again with the rest.
    Narrowed(ModifyOp),
    /// Rejected, with the toast's text.
    Rejected(String),
    /// Its payload does not parse: it can never run.
    Unreadable,
}

/// What a done op leaves behind.
#[derive(Debug)]
enum Landing {
    /// A label change: kept on top of reads that were in flight.
    Modify(ModifyOp),
    /// A label made here, and Gmail's id for it.
    Label { local: LabelId, gmail: LabelId },
    /// A draft written to Gmail, and Gmail's ids for it; `None` when there
    /// was nothing to write.
    Draft {
        draft: DraftId,
        written: Option<Box<sm_gmail::Draft>>,
    },
    /// Nothing more to write.
    Plain,
}

/// Runs `op` once and writes what came of it. Nothing is written when Gmail
/// gave no clear answer: the op stays as it was, to run again.
pub(super) async fn run_op(ctx: &AccountCtx, op: OpRecord) -> Result<(), Failure> {
    // A draft write stays in progress until its Gmail ids are stored.
    let mut writing: Option<WritingDraft> = None;
    let answer = match op.kind {
        OpKind::Modify => match op.payload::<ModifyOp>() {
            Ok(payload) => modify(ctx, payload).await?,
            Err(e) => {
                tracing::error!(op = %op.id, error = %e, "a queued op does not parse; dropping it");
                Answer::Unreadable
            }
        },
        OpKind::LabelCreate => match op.payload::<LabelCreateOp>() {
            Ok(payload) => create_label(ctx, payload).await?,
            Err(e) => {
                tracing::error!(op = %op.id, error = %e, "a queued op does not parse; dropping it");
                Answer::Unreadable
            }
        },
        OpKind::DraftUpsert => {
            if let Some(draft) = op.draft {
                let (answer, guard) = draft_upsert(ctx, draft).await?;
                writing = guard;
                answer
            } else {
                tracing::error!(op = %op.id, "a draft upsert names no draft; dropping it");
                Answer::Unreadable
            }
        }
        OpKind::DraftDelete => {
            if let Some(gmail) = &op.gmail_draft {
                match delete_draft(ctx, gmail).await {
                    Ok(()) => Answer::Done(Landing::Plain),
                    Err(e) => refused(e)?,
                }
            } else if let Some(local) = &op.draft {
                match delete_copies(ctx, local).await {
                    Ok(()) => Answer::Done(Landing::Plain),
                    Err(WriteFailure::Gmail(e)) => refused(e)?,
                    Err(WriteFailure::Store(e)) => return Err(Failure::Store(e)),
                    Err(WriteFailure::Local(message)) => Answer::Rejected(message),
                }
            } else {
                tracing::error!(op = %op.id, "a draft delete names no draft; dropping it");
                Answer::Unreadable
            }
        }
        OpKind::TrashMessage => match op.payload::<GmailMessageId>() {
            Ok(message) => trash_message(ctx, message).await?,
            Err(e) => {
                tracing::error!(op = %op.id, error = %e, "a queued op does not parse; dropping it");
                Answer::Unreadable
            }
        },
    };
    ctx.health.succeeded();
    let written = write(ctx, op.id, answer).await.map_err(Failure::Store);
    if let Some(writing) = writing {
        let draft = writing.draft().clone();
        drop(writing);
        // A send of the draft waited for this write to end.
        wake_outbox_if_sending(ctx, draft);
    }
    written
}

/// `messages.batchModify`, in chunks Gmail takes. A 404 narrows the op to
/// the messages that still exist.
async fn modify(ctx: &AccountCtx, op: ModifyOp) -> Result<Answer, Failure> {
    if op.messages.is_empty() || (op.add.is_empty() && op.remove.is_empty()) {
        return Ok(Answer::Done(Landing::Modify(op)));
    }
    let account = ctx.account;
    let ids = ctx
        .read(move |r| r.labels().gmail_ids(account))
        .await
        .map_err(Failure::Store)?;
    // A label with no Gmail id yet keeps its own: Gmail rejects it, and the
    // change rolls back.
    let gmail = |set: &LabelSet| -> Vec<LabelId> {
        set.iter()
            .map(|l| ids.get(l).unwrap_or(l).clone())
            .collect()
    };
    let (add, remove) = (gmail(&op.add), gmail(&op.remove));
    for chunk in op.messages.chunks(BATCH_MODIFY_MAX) {
        match ctx
            .gmail
            .batch_modify(Priority::P1, chunk, &add, &remove)
            .await
        {
            Ok(()) => {}
            Err(e) if *e.outcome() == Outcome::NotFound => return narrow(ctx, op).await,
            Err(e) => return refused(e),
        }
    }
    Ok(Answer::Done(Landing::Modify(op)))
}

/// After a 404 from `batchModify`, which does not say which id is missing:
/// checks each message with `messages.get` (`format=minimal`) and keeps the
/// ones Gmail still has (SPEC Gmail errors). With none left, the op is done.
async fn narrow(ctx: &AccountCtx, mut op: ModifyOp) -> Result<Answer, Failure> {
    let checks = join_all(
        op.messages
            .iter()
            .map(|id| ctx.gmail.get_message(Priority::P1, id, Format::Minimal)),
    )
    .await;
    let mut alive = Vec::with_capacity(op.messages.len());
    for (id, check) in op.messages.iter().zip(checks) {
        match check {
            Ok(_) => alive.push(id.clone()),
            Err(e) if *e.outcome() == Outcome::NotFound => {}
            Err(e) => return Err(Failure::Gmail(e)),
        }
    }
    if alive.is_empty() {
        return Ok(Answer::Done(Landing::Modify(op)));
    }
    if alive.len() == op.messages.len() {
        // Every message is there, so the 404 was about a label.
        return Ok(Answer::Rejected(
            "Gmail rejected a change: a label it names is gone".to_owned(),
        ));
    }
    op.messages = alive;
    Ok(Answer::Narrowed(op))
}

/// `labels.create`. A 409 means the name exists on Gmail: that label is
/// used, as its id from `labels.list` (SPEC Labels). Not a rejection.
async fn create_label(ctx: &AccountCtx, op: LabelCreateOp) -> Result<Answer, Failure> {
    let created = ctx.gmail.create_label(Priority::P1, &op.name).await;
    let gmail = match created {
        Ok(label) => label.id,
        Err(e) if *e.outcome() == Outcome::Conflict => {
            let listed = ctx
                .gmail
                .list_labels(Priority::P1)
                .await
                .map_err(Failure::Gmail)?;
            match listed
                .into_iter()
                .find(|l| l.name.eq_ignore_ascii_case(&op.name))
            {
                Some(label) => label.id,
                None => {
                    return Ok(Answer::Rejected(
                        "Gmail rejected a new label: the name is taken".to_owned(),
                    ));
                }
            }
        }
        Err(e) => return refused(e),
    };
    Ok(Answer::Done(Landing::Label {
        local: op.label,
        gmail,
    }))
}

/// A draft upsert (SPEC Drafts), and the write in progress to end once its
/// answer is stored. Whatever came of it, a send of the draft waiting for
/// this write to end looks again.
async fn draft_upsert(
    ctx: &AccountCtx,
    draft: DraftId,
) -> Result<(Answer, Option<WritingDraft>), Failure> {
    let upserted = upsert(ctx, &draft).await;
    // A written draft's write ends once its ids are stored (`run_op`).
    if !matches!(upserted, Ok(Upserted::Written { .. })) {
        wake_outbox_if_sending(ctx, draft.clone());
    }
    match upserted {
        Ok(Upserted::Written {
            draft: written,
            writing,
        }) => Ok((
            Answer::Done(Landing::Draft {
                draft,
                written: Some(written),
            }),
            Some(writing),
        )),
        Ok(Upserted::Nothing) => Ok((
            Answer::Done(Landing::Draft {
                draft,
                written: None,
            }),
            None,
        )),
        Ok(Upserted::Busy) => Err(Failure::Busy),
        Err(WriteFailure::Gmail(e)) => Ok((refused(e)?, None)),
        Err(WriteFailure::Local(message)) => Ok((
            Answer::Rejected(format!("The draft can't be saved to Gmail: {message}")),
            None,
        )),
        Err(WriteFailure::Store(e)) => Err(Failure::Store(e)),
    }
}

/// Wakes the account's outbox if `draft` is being sent: its send waits for
/// the draft's write to end. Through the writer, so it reaches the account's
/// current worker even if this one was replaced.
fn wake_outbox_if_sending(ctx: &AccountCtx, draft: DraftId) {
    let account = ctx.account;
    ctx.writer().queue(
        WritePriority::Sync,
        "wake the outbox after an upsert",
        move |tx, state| {
            if tx.read().drafts().get(&draft)?.is_some_and(|d| d.in_outbox) {
                state.effect(Effect::Outbox(account));
            }
            Ok(())
        },
    );
}

/// `messages.trash` of a sent message whose send was undone. One Gmail no
/// longer has is gone already.
async fn trash_message(ctx: &AccountCtx, message: GmailMessageId) -> Result<Answer, Failure> {
    match ctx.gmail.trash_message(Priority::P1, &message).await {
        // Kept on top of reads that were in flight, as a modify is.
        Ok(_) => Ok(Answer::Done(Landing::Modify(ModifyOp::trash(message)))),
        Err(e) if *e.outcome() == Outcome::NotFound => Ok(Answer::Done(Landing::Plain)),
        Err(e) => refused(e),
    }
}

/// A failed call: a rejection is an answer; anything else leaves the op
/// queued (SPEC Gmail errors: only a rejection rolls an action back).
fn refused(e: GmailError) -> Result<Answer, Failure> {
    match e.outcome() {
        Outcome::Rejected {
            status, message, ..
        } => Ok(Answer::Rejected(format!(
            "Gmail rejected a change ({status}): {message}"
        ))),
        Outcome::NotFound | Outcome::Conflict => Ok(Answer::Rejected(format!(
            "Gmail rejected a change: {}",
            e.outcome()
        ))),
        Outcome::Offline
        | Outcome::NoResponse
        | Outcome::Unauthorized
        | Outcome::InvalidGrant
        | Outcome::RateLimited { .. }
        | Outcome::ServerError { .. } => Err(Failure::Gmail(e)),
    }
}

/// Writes a run's answer. An op gone meanwhile (its account removed)
/// writes nothing.
async fn write(ctx: &AccountCtx, id: OpId, answer: Answer) -> Result<(), Error> {
    let (account, now, lane) = (ctx.account, ctx.clock().now(), Arc::clone(&ctx.lane));
    ctx.writer()
        .call(WritePriority::Sync, move |tx, state| match answer {
            Answer::Rejected(message) => reject(tx, id, message, now),
            Answer::Unreadable => tx.ops().delete(id),
            // Stored even when a newer save replaced the op: the next write
            // updates this Gmail draft instead of making another.
            Answer::Done(Landing::Draft { draft, written }) => {
                if let Some(written) = &written {
                    let ids = GmailIds::of(written);
                    let made = ids.draft.clone();
                    // Discarded while its upsert was in flight: the Gmail
                    // draft the upsert made goes too, or sync would bring
                    // it back.
                    if !store_ids(tx, &draft, ids)? {
                        delete_gmail_draft(tx, state, account, &made, now)?;
                    }
                }
                tx.ops().delete(id)
            }
            _ if tx.read().ops().get(id)?.is_none() => Ok::<_, StoreError>(()),
            Answer::Narrowed(op) => tx.ops().set_payload(id, &op),
            Answer::Done(Landing::Modify(op)) => {
                lane.landed.push(op);
                tx.ops().delete(id)
            }
            Answer::Done(Landing::Plain) => tx.ops().delete(id),
            Answer::Done(Landing::Label { local, gmail }) => {
                let used = tx.labels().assign_gmail_id(account, &local, &gmail)?;
                if used != local {
                    relabel(tx, account, &local, &used)?;
                }
                lane.label_mapped();
                tx.ops().delete(id)
            }
        })
        .await?;
    Ok(())
}
