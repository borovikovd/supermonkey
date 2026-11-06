//! Telling the user why the core rejected an action (SPEC UI state,
//! Actions): a notice on the action's account, shown as a toast. A refused
//! send's notice carries its draft, so the draft comes back in a composer as
//! after a send Gmail rejected.

use ub_domain::{AccountId, DraftId, Millis, NoticeKind};
use ub_ipc::{Action, DraftSave};
use ub_store::{NewNotice, StoreError, WriteTx};

use super::ActionError;
use crate::drafts::lifecycle;

/// Who hears of an action's rejection.
pub(super) enum Owner {
    /// Nobody: the UI's own bookkeeping, which the user never asked for.
    /// Its rejection is a bug, and only logged.
    Nobody,
    /// The account the action was for.
    Account(AccountId),
    /// The account of the draft the action was for.
    Draft(DraftId),
}

/// Who hears of `action`'s rejection. Taken before the action runs, since
/// running it consumes it.
pub(super) fn owner(action: &Action) -> Owner {
    match action {
        Action::Reset
        | Action::Subscribe { .. }
        | Action::Unsubscribe { .. }
        | Action::MoreResults { .. }
        | Action::AckNotice { .. }
        | Action::CloseToast { .. } => Owner::Nobody,
        Action::SetSetting { account, .. } => account.map_or(Owner::Nobody, Owner::Account),
        Action::Modify(modify) => Owner::Account(modify.account),
        Action::Undo { account, .. }
        | Action::CreateLabel { account, .. }
        | Action::Pin { account, .. }
        | Action::SaveSplits { account, .. }
        | Action::RemoveAccount { account }
        // A fresh composer's first save: its draft is not stored yet.
        | Action::SaveDraft {
            save: DraftSave::Create { account, .. },
            ..
        } => Owner::Account(*account),
        Action::SaveDraft { draft, .. }
        | Action::CloseComposer { draft }
        | Action::SendDraft { draft }
        | Action::DiscardDraft { draft } => Owner::Draft(draft.clone()),
    }
}

/// Leaves a notice saying why an action was rejected, in the batch's
/// transaction after the action's own writes rolled back. An owner whose
/// account or draft is gone hears nothing: there is nowhere to show it.
pub(super) fn notify(
    tx: &mut WriteTx<'_>,
    owner: Owner,
    error: &ActionError,
    now: Millis,
) -> Result<(), StoreError> {
    let (account, draft) = match owner {
        Owner::Nobody => return Ok(()),
        Owner::Account(account) => (account, None),
        Owner::Draft(id) => match tx.read().drafts().get(&id)? {
            Some(record) => (record.draft.account, Some(record.draft)),
            None => return Ok(()),
        },
    };
    if tx.read().accounts().get(account)?.is_none() {
        return Ok(());
    }
    let (kind, draft, thread) = match (error, draft) {
        (ActionError::Unsendable(_), Some(mut draft)) => {
            lifecycle::keep(&mut draft);
            tx.drafts().put(&draft)?;
            (NoticeKind::SendFailed, Some(draft.id), draft.thread)
        }
        _ => (NoticeKind::Rejected, None, None),
    };
    tx.notices().add(&NewNotice {
        account,
        kind,
        message: error.message(),
        draft,
        thread,
        created_at: now,
    })?;
    Ok(())
}
