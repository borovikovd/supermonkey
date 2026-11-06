//! Split Inbox pins and the splits sheet (SPEC Split Inbox).

use ub_domain::{AccountId, Pin, PinKind, SplitId, SplitKind, UndoEntryId, check_split_rule};
use ub_ipc::SplitInfo;
use ub_store::{SplitEdit, UndoAction};

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

/// `saveSplits`: the splits sheet. Deletes the custom splits in `deleted`
/// with their pins, saves the rest in the order given, and re-routes the
/// Inbox. A custom split's rule must parse strictly; a blank one matches
/// nothing. Important and Other can't be deleted, and Other stays on.
pub(super) fn save_splits(
    ctx: &mut ActionCtx<'_, '_>,
    account: AccountId,
    splits: Vec<SplitInfo>,
    deleted: &[SplitId],
) -> Result<(), ActionError> {
    if ctx.tx.read().accounts().get(account)?.is_none() {
        return Err(ActionError::Invalid("no such account"));
    }
    let mut edits = Vec::with_capacity(splits.len());
    for split in splits {
        if split.kind == SplitKind::Custom {
            check_split_rule(&split.query)?;
        }
        edits.push(SplitEdit {
            id: split.id,
            name: split.name,
            kind: split.kind,
            query: split.query,
            enabled: split.enabled,
            hide_when_empty: split.hide_when_empty,
        });
    }
    ctx.tx.splits().save(account, &edits, deleted)?;
    Ok(())
}
