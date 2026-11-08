//! Telling the page why the core refused an action (SPEC UI state,
//! Actions): the commit's channel message lists the action's number with a
//! sentence for the user. A send refused because its draft can't go as it
//! is keeps the draft, which the page brings back in a composer.

use sm_domain::{ActionNo, DraftId};
use sm_ipc::{Action, Rejection};
use sm_store::{StoreError, WriteTx};

use super::ActionError;
use crate::drafts::lifecycle;
use crate::writer::WriterState;

/// The draft `action` sends, taken before the action runs, since running it
/// consumes it.
pub(super) fn sent_draft(action: &Action) -> Option<DraftId> {
    match action {
        Action::SendDraft { draft } => Some(draft.clone()),
        _ => None,
    }
}

/// Records action `n`'s refusal for the commit's channel message, in the
/// batch's transaction after the action's own writes rolled back. A refused
/// send's draft is kept: it is never discarded as untouched.
pub(super) fn refuse(
    tx: &mut WriteTx<'_>,
    state: &mut WriterState,
    n: ActionNo,
    sent: Option<DraftId>,
    error: &ActionError,
) -> Result<(), StoreError> {
    if let (ActionError::Unsendable(_), Some(id)) = (error, sent)
        && let Some(record) = tx.read().drafts().get(&id)?
    {
        let mut draft = record.draft;
        lifecycle::keep(&mut draft);
        tx.drafts().put(&draft)?;
    }
    state.refuse(Rejection {
        n,
        message: error.message(),
        split: error.split(),
    });
    Ok(())
}
