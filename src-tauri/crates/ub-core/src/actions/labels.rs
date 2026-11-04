//! Creating a label from the `l` picker (SPEC Labels).

use ub_domain::{AccountId, LabelId};
use ub_ipc::ModifyAction;
use ub_store::{NewOp, OpKind};

use super::{ActionCtx, ActionError, mail};
use crate::effects::Effect;
use crate::ops::LabelCreateOp;

/// `createLabel`: a user label under the UI's id, at once, with a queued
/// `labels.create` that gives it Gmail's id when it replays. `apply`, the
/// label's first use, goes in the same action. A label the account already
/// has by that name is used instead.
pub(super) fn create_label(
    ctx: &mut ActionCtx<'_, '_>,
    account: AccountId,
    id: &LabelId,
    name: &str,
    apply: Option<ModifyAction>,
) -> Result<(), ActionError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ActionError::Invalid("a label needs a name"));
    }
    let reads = ctx.tx.read();
    if reads.accounts().get(account)?.is_none() {
        return Err(ActionError::Invalid("no such account"));
    }
    if reads.labels().get(account, id)?.is_some() {
        return Err(ActionError::Invalid("the label id is taken"));
    }
    let label = if let Some(existing) = reads.labels().by_name(account, name)? {
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
            "a label is applied in its own account",
        ));
    }
    for ids in [&mut apply.add, &mut apply.remove] {
        for l in ids.iter_mut().filter(|l| *l == id) {
            l.clone_from(&label);
        }
    }
    mail::modify(ctx, apply)
}
