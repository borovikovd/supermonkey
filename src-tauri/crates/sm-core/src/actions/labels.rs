use sm_domain::{AccountId, LabelId};
use sm_ipc::ModifyAction;
use sm_store::{NewOp, OpKind};

use super::{ActionCtx, ActionError, mail};
use crate::effects::Effect;
use crate::ops::LabelCreateOp;

pub(super) fn create_label(
    ctx: &mut ActionCtx<'_, '_>,
    account: AccountId,
    id: LabelId,
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
    if reads.labels().get(account, &id)?.is_some() {
        return Err(ActionError::Invalid(
            "Couldn't create that label. Try again.",
        ));
    }
    let existing = reads.labels().by_name(account, name)?.filter(|l| !l.system);
    let rename = if let Some(existing) = existing {
        Some((id, existing.id))
    } else {
        queue_create(ctx, account, id, name)?;
        None
    };
    let Some(mut apply) = apply else {
        return Ok(());
    };
    if apply.account != account {
        return Err(ActionError::Invalid(
            "A label applies only in its own account",
        ));
    }
    if let Some((requested, existing)) = &rename {
        retarget(&mut apply, requested, existing);
    }
    mail::modify(ctx, apply)
}

fn queue_create(
    ctx: &mut ActionCtx<'_, '_>,
    account: AccountId,
    id: LabelId,
    name: &str,
) -> Result<(), ActionError> {
    ctx.tx.labels().create_local(account, &id, name)?;
    ctx.tx.ops().push(&NewOp {
        account,
        action: Some(ctx.n),
        kind: OpKind::LabelCreate,
        draft: None,
        gmail_draft: None,
        payload: &LabelCreateOp {
            label: id,
            name: name.to_owned(),
        },
        created_at: ctx.now,
    })?;
    ctx.state.effect(Effect::Ops(account));
    Ok(())
}

fn retarget(apply: &mut ModifyAction, requested: &LabelId, existing: &LabelId) {
    for ids in [&mut apply.add, &mut apply.remove] {
        for l in ids.iter_mut().filter(|l| *l == requested) {
            l.clone_from(existing);
        }
    }
}
