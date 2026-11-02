//! Drafts (SPEC Compose → Drafts). Filled in with the composer.

use crate::model::*;
use crate::writer::Ctx;

pub fn save(_ctx: &mut Ctx, _draft: &str, _save: DraftSave) -> anyhow::Result<()> {
    anyhow::bail!("drafts are not built yet")
}

pub fn close(_ctx: &mut Ctx, _draft: &str) -> anyhow::Result<()> {
    Ok(())
}

pub fn discard(_ctx: &mut Ctx, _draft: &str) -> anyhow::Result<()> {
    anyhow::bail!("drafts are not built yet")
}
