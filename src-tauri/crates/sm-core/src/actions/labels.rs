//! Creating a label from the `l` picker (SPEC Labels).

use sm_domain::{AccountId, LabelId};
use sm_ipc::ModifyAction;
use sm_store::{NewOp, OpKind};

use super::{ActionCtx, ActionError, mail};
use crate::effects::Effect;
use crate::ops::LabelCreateOp;

/// `createLabel`: a user label under the UI's id, at once, with a queued
/// `labels.create` that gives it Gmail's id when it replays. `apply`, the
/// label's first use, goes in the same action. A user label the account
/// already has by that name is used instead.
pub(super) fn create_label(
    ctx: &mut ActionCtx<'_, '_>,
    account: AccountId,
    id: &LabelId,
    name: &str,
    apply: Option<ModifyAction>,
) -> Result<(), ActionError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ActionError::Invalid("A label needs a name"));
    }
    let reads = ctx.tx.read();
    if reads.accounts().get(account)?.is_none() {
        return Err(ActionError::Invalid(
            "That account is no longer in Supermonkey",
        ));
    }
    if reads.labels().get(account, id)?.is_some() {
        return Err(ActionError::Invalid(
            "Couldn't create that label. Try again.",
        ));
    }
    // A system label's name is Gmail's to refuse, as a rejection.
    let existing = reads.labels().by_name(account, name)?.filter(|l| !l.system);
    let label = if let Some(existing) = existing {
        existing.id
    } else {
        ctx.tx.labels().create_local(account, id, name)?;
        ctx.tx.ops().push(&NewOp {
            account,
            action: Some(ctx.n),
            kind: OpKind::LabelCreate,
            draft: None,
            gmail_draft: None,
            payload: &LabelCreateOp {
                label: id.clone(),
                name: name.to_owned(),
            },
            created_at: ctx.now,
        })?;
        ctx.state.effect(Effect::Ops(account));
        id.clone()
    };
    let Some(mut apply) = apply else {
        return Ok(());
    };
    if apply.account != account {
        return Err(ActionError::Invalid(
            "A label applies only in its own account",
        ));
    }
    for ids in [&mut apply.add, &mut apply.remove] {
        for l in ids.iter_mut().filter(|l| *l == id) {
            l.clone_from(&label);
        }
    }
    mail::modify(ctx, apply)
}
