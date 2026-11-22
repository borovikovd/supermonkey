use sm_domain::{DraftId, GmailDraftId};
use sm_gmail::{GmailError, Outcome, Priority};

use super::WriteFailure;
use super::parts::download_parts;
use super::write::{load, write};
use crate::drafts::WritingDraft;
use crate::drafts::lifecycle;
use crate::workers::account::AccountCtx;

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
        let taken = load(&ctx.deps.pool, id.clone())
            .await?
            .is_none_or(|r| r.in_outbox);
        return Ok(if taken {
            Upserted::Nothing
        } else {
            Upserted::Busy
        });
    };
    let Some(record) = load(&ctx.deps.pool, id.clone()).await? else {
        return Ok(Upserted::Nothing);
    };
    if record.in_outbox {
        return Ok(Upserted::Nothing);
    }
    let draft = if lifecycle::awaits_parts(&record.draft) {
        download_parts(ctx, id.clone()).await?;
        let Some(record) = load(&ctx.deps.pool, id.clone()).await? else {
            return Ok(Upserted::Nothing);
        };
        record.draft
    } else {
        record.draft
    };
    let Some(written) = write(ctx, draft).await? else {
        return Ok(Upserted::Nothing);
    };
    Ok(Upserted::Written {
        draft: Box::new(written),
        writing,
    })
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
