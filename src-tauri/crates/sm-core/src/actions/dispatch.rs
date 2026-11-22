use sm_domain::{AccountId, ActionNo, Millis};
use sm_ipc::Action;
use sm_store::WriteTx;

use super::{ActionError, accounts, compose, labels, mail, settings, splits, subscriptions};
use crate::writer::WriterState;

pub(crate) struct ActionCtx<'a, 'c> {
    pub(crate) tx: &'a mut WriteTx<'c>,
    pub(crate) state: &'a mut WriterState,
    pub(crate) n: ActionNo,
    pub(crate) now: Millis,
}

impl ActionCtx<'_, '_> {
    pub(super) fn require_account(&self, account: AccountId) -> Result<(), ActionError> {
        if self.tx.read().accounts().get(account)?.is_none() {
            return Err(ActionError::Invalid(
                "That account is no longer in Supermonkey",
            ));
        }
        Ok(())
    }
}

pub(super) fn apply(ctx: &mut ActionCtx<'_, '_>, action: Action) -> Result<(), ActionError> {
    match action {
        Action::Reset => subscriptions::reset(ctx),
        Action::Subscribe { sub, key } => subscriptions::subscribe(ctx, sub, key),
        Action::Unsubscribe { sub } => {
            subscriptions::unsubscribe(ctx, sub);
            Ok(())
        }
        Action::MoreResults { sub } => subscriptions::more_results(ctx, sub),
        Action::SetSetting(setting) => settings::set_setting(ctx, setting),
        Action::AckNotice { notice } => Ok(ctx.tx.notices().delete(notice)?),
        Action::CloseToast { outbox } => Ok(crate::outbox::close_toast(ctx.tx, ctx.state, outbox)?),
        Action::Modify(modify) => mail::modify(ctx, modify),
        Action::Undo { account, entry } => mail::undo(ctx, account, entry),
        Action::CreateLabel {
            account,
            id,
            name,
            apply,
        } => labels::create_label(ctx, account, id, &name, apply),
        Action::SaveDraft { draft, save } => compose::save_draft(ctx, draft, save),
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

pub(super) const fn kind(action: &Action) -> &'static str {
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
