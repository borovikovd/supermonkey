use std::sync::Arc;

use futures_util::future::join_all;
use serde::de::DeserializeOwned;
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

const BATCH_MODIFY_MAX: usize = 1000;

#[derive(Debug)]
pub(super) enum Failure {
    Gmail(GmailError),
    Store(Error),
    Busy,
}

#[derive(Debug)]
enum Answer {
    Done(Landing),
    Narrowed(ModifyOp),
    Rejected(String),
    Unreadable,
}

#[derive(Debug)]
enum Landing {
    Modify(ModifyOp),
    Label {
        local: LabelId,
        gmail: LabelId,
    },
    Draft {
        draft: DraftId,
        written: Option<Box<sm_gmail::Draft>>,
    },
    Plain,
}

pub(super) async fn run_op(ctx: &AccountCtx, op: OpRecord) -> Result<(), Failure> {
    let id = op.id;
    let (answer, writing) = answer(ctx, op).await?;
    ctx.health.succeeded();
    let written = write(ctx, id, answer).await.map_err(Failure::Store);
    if let Some(writing) = writing {
        let draft = writing.draft().clone();
        drop(writing);
        wake_outbox_if_sending(ctx, draft);
    }
    written
}

async fn answer(ctx: &AccountCtx, op: OpRecord) -> Result<(Answer, Option<WritingDraft>), Failure> {
    let answer = match op.kind {
        OpKind::Modify => match parsed::<ModifyOp>(&op) {
            Some(payload) => modify(ctx, payload).await?,
            None => Answer::Unreadable,
        },
        OpKind::LabelCreate => match parsed::<LabelCreateOp>(&op) {
            Some(payload) => create_label(ctx, payload).await?,
            None => Answer::Unreadable,
        },
        OpKind::DraftUpsert => return draft_upsert(ctx, op.id, op.draft).await,
        OpKind::DraftDelete => draft_delete(ctx, &op).await?,
        OpKind::TrashMessage => match parsed::<GmailMessageId>(&op) {
            Some(message) => trash_message(ctx, message).await?,
            None => Answer::Unreadable,
        },
    };
    Ok((answer, None))
}

fn parsed<P: DeserializeOwned>(op: &OpRecord) -> Option<P> {
    match op.payload::<P>() {
        Ok(payload) => Some(payload),
        Err(e) => {
            tracing::error!(op = %op.id, error = %e, "a queued op does not parse; dropping it");
            None
        }
    }
}

async fn modify(ctx: &AccountCtx, op: ModifyOp) -> Result<Answer, Failure> {
    if op.messages.is_empty() || (op.add.is_empty() && op.remove.is_empty()) {
        return Ok(Answer::Done(Landing::Modify(op)));
    }
    let account = ctx.account;
    let ids = ctx
        .read(move |r| r.labels().gmail_ids(account))
        .await
        .map_err(Failure::Store)?;
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

async fn narrow(ctx: &AccountCtx, mut op: ModifyOp) -> Result<Answer, Failure> {
    let checks = join_all(
        op.messages
            .iter()
            .map(|id| ctx.gmail.get_message(Priority::P1, id, Format::Minimal)),
    )
    .await;
    let mut alive = Vec::with_capacity(op.messages.len());
    for check in checks {
        match check {
            Ok(_) => alive.push(true),
            Err(e) if *e.outcome() == Outcome::NotFound => alive.push(false),
            Err(e) => return Err(Failure::Gmail(e)),
        }
    }
    let left = alive.iter().filter(|alive| **alive).count();
    if left == 0 {
        return Ok(Answer::Done(Landing::Modify(op)));
    }
    if left == op.messages.len() {
        return Ok(Answer::Rejected(
            "Gmail rejected a change: a label it names is gone".to_owned(),
        ));
    }
    let mut alive = alive.into_iter();
    op.messages.retain(|_| alive.next().unwrap_or(false));
    Ok(Answer::Narrowed(op))
}

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

async fn draft_upsert(
    ctx: &AccountCtx,
    op: OpId,
    draft: Option<DraftId>,
) -> Result<(Answer, Option<WritingDraft>), Failure> {
    let Some(draft) = draft else {
        tracing::error!(%op, "a draft upsert names no draft; dropping it");
        return Ok((Answer::Unreadable, None));
    };
    let upserted = upsert(ctx, &draft).await;
    if !matches!(upserted, Ok(Upserted::Written { .. })) {
        wake_outbox_if_sending(ctx, draft.clone());
    }
    upsert_answer(draft, upserted)
}

fn upsert_answer(
    draft: DraftId,
    upserted: Result<Upserted, WriteFailure>,
) -> Result<(Answer, Option<WritingDraft>), Failure> {
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

async fn draft_delete(ctx: &AccountCtx, op: &OpRecord) -> Result<Answer, Failure> {
    if let Some(gmail) = &op.gmail_draft {
        return match delete_draft(ctx, gmail).await {
            Ok(()) => Ok(Answer::Done(Landing::Plain)),
            Err(e) => refused(e),
        };
    }
    let Some(local) = &op.draft else {
        tracing::error!(op = %op.id, "a draft delete names no draft; dropping it");
        return Ok(Answer::Unreadable);
    };
    delete_local(ctx, op.id, local).await
}

async fn delete_local(ctx: &AccountCtx, id: OpId, local: &DraftId) -> Result<Answer, Failure> {
    match delete_copies(ctx, local).await {
        Ok(None) => Ok(Answer::Done(Landing::Plain)),
        Ok(Some(unread)) => {
            ctx.writer()
                .call(WritePriority::Sync, move |tx, _| tx.ops().requeue(id))
                .await
                .map_err(|e| Failure::Store(e.into()))?;
            Err(Failure::Gmail(unread))
        }
        Err(WriteFailure::Gmail(e)) => refused(e),
        Err(WriteFailure::Store(e)) => Err(Failure::Store(e)),
        Err(WriteFailure::Local(message)) => Ok(Answer::Rejected(message)),
    }
}

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

async fn trash_message(ctx: &AccountCtx, message: GmailMessageId) -> Result<Answer, Failure> {
    match ctx.gmail.trash_message(Priority::P1, &message).await {
        Ok(_) => Ok(Answer::Done(Landing::Modify(ModifyOp::trash(message)))),
        Err(e) if *e.outcome() == Outcome::NotFound => Ok(Answer::Done(Landing::Plain)),
        Err(e) => refused(e),
    }
}

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

async fn write(ctx: &AccountCtx, id: OpId, answer: Answer) -> Result<(), Error> {
    let (account, lane) = (ctx.account, Arc::clone(&ctx.lane));
    ctx.writer()
        .call(WritePriority::Sync, move |tx, state| match answer {
            Answer::Rejected(message) => reject(tx, id, message),
            Answer::Unreadable => tx.ops().delete(id),
            Answer::Done(Landing::Draft { draft, written }) => {
                if let Some(written) = written {
                    let ids = GmailIds::of(*written);
                    let made = ids.draft.clone();
                    if !store_ids(tx, &draft, ids)? {
                        delete_gmail_draft(tx, state, account, &made)?;
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
                if let Some(used) = tx.labels().assign_gmail_id(account, &local, &gmail)? {
                    relabel(tx, account, &local, &used)?;
                }
                lane.label_mapped();
                tx.ops().delete(id)
            }
        })
        .await?;
    Ok(())
}
