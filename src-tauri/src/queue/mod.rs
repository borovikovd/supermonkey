//! The Gmail op queue (SPEC Offline). Ops are SQLite rows replayed in order
//! per account by the async runner in `runner.rs`.

pub mod runner;

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

/// Message ids with an op still pending, for the snooze rule after a history batch.
pub fn pending_message_ids(
    conn: &rusqlite::Connection,
    account: AccountId,
) -> anyhow::Result<std::collections::HashSet<String>> {
    let mut stmt =
        conn.prepare_cached("SELECT payload FROM ops WHERE account_id = ?1 AND kind = 'modify'")?;
    let mut out = std::collections::HashSet::new();
    for p in stmt.query_map([account], |r| r.get::<_, String>(0))? {
        let v: serde_json::Value = serde_json::from_str(&p?).unwrap_or_default();
        if let Some(a) = v["messages"].as_array() {
            out.extend(a.iter().filter_map(|m| m.as_str().map(str::to_string)));
        }
    }
    Ok(out)
}
