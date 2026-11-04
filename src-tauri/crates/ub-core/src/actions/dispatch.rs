//! One action to its handler.

use ub_ipc::Action;
use ub_store::WriteTx;

use super::{
    ActionError, accounts, compose, mail, notices, outbox, settings, splits, subscriptions,
};
use crate::writer::WriterState;

/// What a handler works with: the open transaction and the writer state.
/// It never sees SQL or the channel.
pub(crate) struct ActionCtx<'a, 'c> {
    /// The batch's transaction, inside this action's savepoint.
    pub(crate) tx: &'a mut WriteTx<'c>,
    /// Subscriptions and effects.
    pub(crate) state: &'a mut WriterState,
}

/// Applies one action.
pub(super) fn apply(ctx: &mut ActionCtx<'_, '_>, action: Action) -> Result<(), ActionError> {
    match action {
        Action::Reset => {
            subscriptions::reset(ctx);
            Ok(())
        }
        Action::Subscribe { sub, key } => subscriptions::subscribe(ctx, sub, key),
        Action::Unsubscribe { sub } => {
            subscriptions::unsubscribe(ctx, sub);
            Ok(())
        }
        Action::MoreResults { sub } => subscriptions::more_results(ctx, &sub),
        Action::SetSetting {
            account,
            key,
            value,
        } => settings::set_setting(ctx, account, &key, &value),
        Action::AckNotice { notice } => notices::ack_notice(ctx, notice),
        Action::CloseToast { outbox } => outbox::close_toast(ctx, outbox),
        Action::Modify(_) => mail::modify(),
        Action::Undo { .. } => mail::undo(),
        Action::CreateLabel { .. } => mail::create_label(),
        Action::SaveDraft { .. } => compose::save_draft(),
        Action::CloseComposer { .. } => compose::close_composer(),
        Action::SendDraft { .. } => compose::send_draft(),
        Action::DiscardDraft { .. } => compose::discard_draft(),
        Action::Pin { .. } => splits::pin(),
        Action::SaveSplits { .. } => splits::save_splits(),
        Action::RemoveAccount { .. } => accounts::remove_account(),
    }
}

/// The action's type, as the UI names it, for logs.
pub(super) fn kind(action: &Action) -> &'static str {
    match action {
        Action::Reset => "reset",
        Action::Subscribe { .. } => "subscribe",
        Action::Unsubscribe { .. } => "unsubscribe",
        Action::Modify(_) => "modify",
        Action::Undo { .. } => "undo",
        Action::CreateLabel { .. } => "createLabel",
        Action::SaveDraft { .. } => "saveDraft",
        Action::CloseComposer { .. } => "closeComposer",
        Action::SendDraft { .. } => "sendDraft",
        Action::DiscardDraft { .. } => "discardDraft",
        Action::CloseToast { .. } => "closeToast",
        Action::AckNotice { .. } => "ackNotice",
        Action::SetSetting { .. } => "setSetting",
        Action::Pin { .. } => "pin",
        Action::SaveSplits { .. } => "saveSplits",
        Action::RemoveAccount { .. } => "removeAccount",
        Action::MoreResults { .. } => "moreResults",
    }
}
