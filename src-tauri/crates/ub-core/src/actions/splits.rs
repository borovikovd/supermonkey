//! Split Inbox pins and the splits sheet (SPEC Split Inbox).
//!
//! Extension point: the splits slice fills in `saveSplits`, validating rules
//! with `ub_domain::Query::parse_rule` before the store saves them.

use ub_domain::{AccountId, Pin, PinKind, SplitId, UndoEntryId};
use ub_store::UndoAction;

use super::{ActionCtx, ActionError};

/// `pin`: routes a sender, domain, or label to a split, re-routing the
/// Inbox. An undo entry keeps the pin it replaced.
pub(super) fn pin(
    ctx: &mut ActionCtx<'_, '_>,
    account: AccountId,
    kind: PinKind,
    value: &str,
    split: SplitId,
) -> Result<(), ActionError> {
    let Some(target) = ctx
        .tx
        .read()
        .splits()
        .get(split)?
        .filter(|s| s.account == account)
    else {
        return Err(ActionError::Invalid("no such split"));
    };
    let Some(pin) = Pin::new(kind, value, split) else {
        return Err(ActionError::Invalid("a pin needs a value"));
    };
    let replaced = ctx.tx.splits().set_pin(account, &pin)?;
    ctx.tx.undo().record(
        account,
        UndoEntryId(ctx.n.0),
        &format!("Moved to {}", target.name),
        &UndoAction::Pin {
            pin_kind: kind,
            value: pin.value().to_owned(),
            split,
            replaced,
        },
    )?;
    Ok(())
}

/// Undoes a pin: the pin it replaced comes back, or, when there was none or
/// its split is gone, the pin goes.
pub(super) fn undo_pin(
    ctx: &mut ActionCtx<'_, '_>,
    account: AccountId,
    kind: PinKind,
    value: &str,
    replaced: Option<SplitId>,
) -> Result<(), ActionError> {
    let earlier = match replaced {
        Some(split) if ctx.tx.read().splits().get(split)?.is_some() => Pin::new(kind, value, split),
        _ => None,
    };
    match earlier {
        Some(pin) => drop(ctx.tx.splits().set_pin(account, &pin)?),
        None => ctx.tx.splits().remove_pin(account, kind, value)?,
    }
    Ok(())
}

/// `saveSplits`: the splits sheet.
pub(super) fn save_splits() -> Result<(), ActionError> {
    Err(ActionError::NotYetImplemented("saveSplits"))
}
