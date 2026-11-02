//! The outbox and undo send (SPEC Send and undo). Filled in with the composer.

use crate::model::*;
use crate::writer::Ctx;

pub fn send(_ctx: &mut Ctx, _n: i64, _draft: &str) -> anyhow::Result<()> {
    anyhow::bail!("send is not built yet")
}

pub fn close_toast(_ctx: &mut Ctx, _outbox: i64) -> anyhow::Result<()> {
    Ok(())
}

pub fn undo_send(_ctx: &mut Ctx, _account: AccountId, _outbox: i64) -> anyhow::Result<()> {
    anyhow::bail!("send is not built yet")
}
