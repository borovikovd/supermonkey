//! The Gmail op queue (SPEC Offline). Ops are SQLite rows replayed in order
//! per account by the engine's op runner (`engine/ops.rs`).

use rusqlite::params;
use serde::Serialize;

use crate::model::AccountId;
use crate::writer::{Ctx, Effect};

pub fn push<T: Serialize>(
    ctx: &mut Ctx<'_>,
    account: AccountId,
    action: Option<i64>,
    kind: &str,
    payload: &T,
) -> anyhow::Result<i64> {
    ctx.tx.execute(
        "INSERT INTO ops (account_id, action, kind, payload, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![account, action, kind, serde_json::to_string(payload)?, ctx.now],
    )?;
    let id = ctx.tx.last_insert_rowid();
    ctx.touch_summary(account);
    ctx.effect(Effect::Ops(account));
    Ok(id)
}
