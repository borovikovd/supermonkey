//! Drafts in composers (SPEC Compose, Drafts, Send and undo; UI state,
//! Actions). Send, discard, and close each close the draft's composer
//! subscription in the same commit.

use sm_domain::DraftId;
use sm_ipc::DraftSave;

use super::{ActionCtx, ActionError};
use crate::drafts::{self, NewDraft};
use crate::outbox;

/// `saveDraft`: a fresh composer's first save, which Rust fills, or its
/// fields after an edit. A save that reaches the writer after its draft was
/// sent or discarded is dropped.
pub(super) fn save_draft(
    ctx: &mut ActionCtx<'_, '_>,
    draft: DraftId,
    save: DraftSave,
) -> Result<(), ActionError> {
    if ctx
        .tx
        .read()
        .drafts()
        .get(&draft)?
        .is_some_and(|d| d.in_outbox)
    {
        return Ok(());
    }
    match save {
        DraftSave::Create {
            account,
            kind,
            reply_all,
            thread,
            target,
            started_on_latest,
        } => drafts::create(
            ctx.tx,
            NewDraft {
                id: draft,
                account,
                kind,
                reply_all,
                thread,
                target,
                started_on_latest,
            },
            ctx.now,
        ),
        DraftSave::Update { fields, rev } => {
            let edit = drafts::Edit { fields, seen: rev };
            Ok(drafts::update(ctx.tx, ctx.state, &draft, edit, ctx.now)?)
        }
    }
}

/// `closeComposer`: the untouched rule, and the composer subscription closes.
pub(super) fn close_composer(
    ctx: &mut ActionCtx<'_, '_>,
    draft: &DraftId,
) -> Result<(), ActionError> {
    drafts::close(ctx.tx, ctx.state, draft, ctx.now)?;
    ctx.state.subs.close_composers(draft);
    Ok(())
}

/// `sendDraft`: queue the draft in the outbox.
pub(super) fn send_draft(ctx: &mut ActionCtx<'_, '_>, draft: &DraftId) -> Result<(), ActionError> {
    outbox::send(ctx.tx, ctx.state, draft, ctx.n, ctx.now)?;
    ctx.state.subs.close_composers(draft);
    Ok(())
}

/// `discardDraft`: delete the draft, here and on Gmail.
pub(super) fn discard_draft(
    ctx: &mut ActionCtx<'_, '_>,
    draft: &DraftId,
) -> Result<(), ActionError> {
    drafts::discard(ctx.tx, ctx.state, draft, ctx.now)?;
    ctx.state.subs.close_composers(draft);
    Ok(())
}
