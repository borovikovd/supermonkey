//! The queued draft ops (SPEC Drafts, Offline): an upsert writes the
//! draft's latest state, and a delete removes a Gmail draft.

use ub_domain::{DraftId, GmailDraftId};
use ub_gmail::{GmailError, Outcome, Priority};

use super::WriteFailure;
use super::parts::download_parts;
use super::write::{load, write};
use crate::workers::account::AccountCtx;

/// What an upsert did.
#[derive(Debug)]
pub(crate) enum Upserted {
    /// Nothing to write: the draft is gone, or the outbox writes it.
    Nothing,
    /// Written: Gmail's draft for it.
    Written(Box<ub_gmail::Draft>),
    /// The outbox is writing it now and the draft is not in the outbox:
    /// run again once that write ends.
    Busy,
}

/// Writes the draft as it is now to Gmail, unless the outbox has taken it
/// (it writes the final version itself). A draft read from Gmail gets its
/// parts first. Two writes of one draft never overlap.
pub(crate) async fn upsert(ctx: &AccountCtx, id: &DraftId) -> Result<Upserted, WriteFailure> {
    let Some(_writing) = ctx.lane.writing.begin(id) else {
        let taken = load_record(ctx, id)
            .await?
            .is_none_or(|in_outbox| in_outbox);
        return Ok(if taken {
            Upserted::Nothing
        } else {
            Upserted::Busy
        });
    };
    if load_record(ctx, id)
        .await?
        .is_none_or(|in_outbox| in_outbox)
    {
        return Ok(Upserted::Nothing);
    }
    if load(ctx, id).await?.is_some_and(|d| d.needs_parts) {
        download_parts(ctx, id).await?;
    }
    let Some(draft) = load(ctx, id).await? else {
        return Ok(Upserted::Nothing);
    };
    Ok(Upserted::Written(Box::new(write(ctx, &draft).await?)))
}

/// Whether the draft is in the outbox, or `None` when it is gone.
async fn load_record(ctx: &AccountCtx, id: &DraftId) -> Result<Option<bool>, WriteFailure> {
    let id = id.clone();
    Ok(ctx
        .read(move |r| r.drafts().get(&id))
        .await?
        .map(|record| record.in_outbox))
}

/// `drafts.delete`. A draft Gmail no longer has is deleted already.
pub(crate) async fn delete_draft(ctx: &AccountCtx, id: &GmailDraftId) -> Result<(), GmailError> {
    match ctx.gmail.delete_draft(Priority::P1, id).await {
        Err(e) if *e.outcome() != Outcome::NotFound => Err(e),
        _ => Ok(()),
    }
}
