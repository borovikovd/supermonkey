use sm_domain::{ActionNo, DraftId};
use sm_ipc::{Action, Rejection};
use sm_store::{StoreError, WriteTx};

use super::ActionError;
use crate::drafts::lifecycle;
use crate::writer::WriterState;

pub(super) fn sent_draft(action: &Action) -> Option<DraftId> {
    if let Action::SendDraft { draft } = action {
        Some(draft.clone())
    } else {
        None
    }
}

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
        reason: error.reason(),
    });
    Ok(())
}
