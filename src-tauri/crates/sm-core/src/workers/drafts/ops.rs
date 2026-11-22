use sm_domain::{DraftId, GmailDraftId};
use sm_gmail::{GmailError, Outcome, Priority};

use super::WriteFailure;
use super::parts::download_parts;
use super::write::{load, write};
use crate::drafts::WritingDraft;
use crate::drafts::lifecycle;
use crate::read::read;
use crate::workers::account::AccountCtx;
use sm_store::ReadPool;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) enum Upserted {
    Nothing,
    Written {
        draft: Box<sm_gmail::Draft>,
        writing: WritingDraft,
    },
    Busy,
}

pub(crate) async fn upsert(ctx: &AccountCtx, id: &DraftId) -> Result<Upserted, WriteFailure> {
    let Some(writing) = ctx.lane.writing.begin(id.clone()) else {
        let taken = load_record(&ctx.deps.pool, id.clone())
            .await?
            .is_none_or(|in_outbox| in_outbox);
        return Ok(if taken {
            Upserted::Nothing
        } else {
            Upserted::Busy
        });
    };
    if load_record(&ctx.deps.pool, id.clone())
        .await?
        .is_none_or(|in_outbox| in_outbox)
    {
        return Ok(Upserted::Nothing);
    }
    if load(&ctx.deps.pool, id.clone())
        .await?
        .is_some_and(|d| lifecycle::awaits_parts(&d))
    {
        download_parts(ctx, id.clone()).await?;
    }
    let Some(draft) = load(&ctx.deps.pool, id.clone()).await? else {
        return Ok(Upserted::Nothing);
    };
    let Some(written) = write(ctx, draft).await? else {
        return Ok(Upserted::Nothing);
    };
    Ok(Upserted::Written {
        draft: Box::new(written),
        writing,
    })
}

async fn load_record(pool: &Arc<ReadPool>, id: DraftId) -> Result<Option<bool>, WriteFailure> {
    Ok(read(pool, move |r| r.drafts().get(&id))
        .await?
        .map(|record| record.in_outbox))
}

pub(crate) async fn delete_copies(
    ctx: &AccountCtx,
    local: &DraftId,
) -> Result<Option<GmailError>, WriteFailure> {
    let copies = super::write::copies_of(ctx, local).await?;
    for copy in copies.ours {
        delete_draft(ctx, &copy.id).await?;
    }
    Ok(copies.unread)
}

pub(crate) async fn delete_draft(ctx: &AccountCtx, id: &GmailDraftId) -> Result<(), GmailError> {
    match ctx.gmail.delete_draft(Priority::P1, id).await {
        Err(e) if *e.outcome() != Outcome::NotFound => Err(e),
        _ => Ok(()),
    }
}
