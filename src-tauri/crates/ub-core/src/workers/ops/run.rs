//! One run of one op: its Gmail calls, then its result written.

use std::sync::Arc;

use futures_util::future::join_all;
use ub_domain::{LabelId, LabelSet};
use ub_gmail::{Format, GmailError, Outcome, Priority};
use ub_store::{OpId, OpKind, OpRecord, Priority as WritePriority, StoreError};

use crate::Error;
use crate::ops::{LabelCreateOp, ModifyOp, reject, relabel};
use crate::workers::account::AccountCtx;

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
    /// No runner handles this kind of op yet.
    NotYet(&'static str),
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
}

/// Runs `op` once and writes what came of it. Nothing is written when Gmail
/// gave no clear answer: the op stays as it was, to run again.
pub(super) async fn run_op(ctx: &AccountCtx, op: OpRecord) -> Result<(), Failure> {
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
        // Extension point: the compose slice runs draft upserts, deletes,
        // and the trash of a sent message (SPEC Drafts, Send and undo).
        OpKind::DraftUpsert | OpKind::DraftDelete | OpKind::TrashMessage => {
            return Err(Failure::NotYet(op.kind.as_str()));
        }
    };
    ctx.health.succeeded();
    write(ctx, op.id, answer).await.map_err(Failure::Store)
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
        .call(WritePriority::Sync, move |tx, _| match answer {
            Answer::Rejected(message) => reject(tx, id, &message, now),
            Answer::Unreadable => tx.ops().delete(id),
            _ if tx.read().ops().get(id)?.is_none() => Ok::<_, StoreError>(()),
            Answer::Narrowed(op) => tx.ops().set_payload(id, &op),
            Answer::Done(Landing::Modify(op)) => {
                lane.landed.push(op);
                tx.ops().delete(id)
            }
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
