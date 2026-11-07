//! One action to its handler.

use ub_domain::{ActionNo, Millis};
use ub_ipc::Action;
use ub_store::WriteTx;

use super::{
    ActionError, accounts, compose, labels, mail, notices, outbox, settings, splits, subscriptions,
};
use crate::writer::WriterState;

/// What a handler works with: the open transaction and the writer state.
/// It never sees SQL or the channel.
pub(crate) struct ActionCtx<'a, 'c> {
    /// The batch's transaction, inside this action's savepoint.
    pub(crate) tx: &'a mut WriteTx<'c>,
    /// Subscriptions and effects.
    pub(crate) state: &'a mut WriterState,
    /// The action's number: the id of the undo entry it makes.
    pub(crate) n: ActionNo,
    /// When its batch was submitted.
    pub(crate) now: Millis,
}

/// Applies one action.
pub(super) fn apply(ctx: &mut ActionCtx<'_, '_>, action: Action) -> Result<(), ActionError> {
    match action {
        Action::Reset => subscriptions::reset(ctx),
        Action::Subscribe { sub, key } => subscriptions::subscribe(ctx, sub, key),
        Action::Unsubscribe { sub } => {
            subscriptions::unsubscribe(ctx, sub);
            Ok(())
        }
        Action::MoreResults { sub } => subscriptions::more_results(ctx, &sub),
        Action::SetSetting(setting) => settings::set_setting(ctx, setting),
        Action::AckNotice { notice } => notices::ack_notice(ctx, notice),
        Action::CloseToast { outbox } => outbox::close_toast(ctx, outbox),
        Action::Modify(modify) => mail::modify(ctx, modify),
        Action::Undo { account, entry } => mail::undo(ctx, account, entry),
        Action::CreateLabel {
            account,
            id,
            name,
            apply,
        } => labels::create_label(ctx, account, &id, &name, apply),
        Action::SaveDraft { draft, save } => compose::save_draft(ctx, &draft, save),
        Action::CloseComposer { draft } => compose::close_composer(ctx, &draft),
        Action::SendDraft { draft } => compose::send_draft(ctx, &draft),
        Action::DiscardDraft { draft } => compose::discard_draft(ctx, &draft),
        Action::Pin {
            account,
            kind,
            value,
            split,
        } => splits::pin(ctx, account, kind, &value, split),
        Action::SaveSplits {
            account,
            splits,
            deleted,
        } => splits::save_splits(ctx, account, splits, &deleted),
        Action::RemoveAccount { account } => accounts::remove_account(ctx, account),
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
        Action::SetSetting(_) => "setSetting",
        Action::Pin { .. } => "pin",
        Action::SaveSplits { .. } => "saveSplits",
        Action::RemoveAccount { .. } => "removeAccount",
        Action::MoreResults { .. } => "moreResults",
    }
}
