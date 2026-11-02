//! Undo history (SPEC Keyboard → Undo). Entries are keyed by the number of
//! the action that made them; the last 50 per account are kept.

use rusqlite::{OptionalExtension, params};

use crate::model::AccountId;
use crate::writer::Ctx;

const KEEP: i64 = 50;

pub fn record(
    ctx: &mut Ctx<'_>,
    n: i64,
    account: AccountId,
    kind: &str,
    label: &str,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    ctx.tx.execute(
        "INSERT OR REPLACE INTO undo_entries (id, account_id, kind, label, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![n, account, kind, label, payload.to_string()],
    )?;
    ctx.tx.execute(
        "DELETE FROM undo_entries WHERE account_id = ?1 AND id NOT IN
           (SELECT id FROM undo_entries WHERE account_id = ?1 ORDER BY id DESC LIMIT ?2)",
        params![account, KEEP],
    )?;
    ctx.touch_summary(account);
    Ok(())
}

/// Undoes one entry. Rejects the action when the entry is gone, so two quick
/// undos never undo two entries by mistake.
pub fn undo(ctx: &mut Ctx<'_>, n: i64, account: AccountId, entry: i64) -> anyhow::Result<()> {
    let Some((kind, payload)) = ctx
        .tx
        .query_row(
            "SELECT kind, payload FROM undo_entries WHERE id = ?1 AND account_id = ?2",
            params![entry, account],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    else {
        anyhow::bail!("undo entry {entry} is gone");
    };
    let payload: serde_json::Value = serde_json::from_str(&payload)?;
    ctx.tx
        .execute("DELETE FROM undo_entries WHERE id = ?1", [entry])?;
    ctx.touch_summary(account);
    match kind.as_str() {
        "mail" => crate::mail::undo(ctx, n, account, &payload),
        "pin" => crate::splits::undo_pin(ctx, account, &payload),
        "send" => crate::outbox::undo_send(ctx, account, payload["outbox"].as_i64().unwrap_or(0)),
        other => anyhow::bail!("unknown undo kind {other}"),
    }
}

/// Drops the send entry for an outbox row (its toast closed after it was sent).
pub fn drop_send_entry(ctx: &mut Ctx<'_>, account: AccountId, outbox: i64) -> anyhow::Result<()> {
    let n = ctx.tx.execute(
        "DELETE FROM undo_entries WHERE account_id = ?1 AND kind = 'send' AND json_extract(payload, '$.outbox') = ?2",
        params![account, outbox],
    )?;
    if n > 0 {
        ctx.touch_summary(account);
    }
    Ok(())
}
