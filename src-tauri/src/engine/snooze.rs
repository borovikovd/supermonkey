//! Snooze waking and the snooze rule (SPEC Snooze).

use std::time::Duration;

use super::EngineRef;
use super::ingest::PendingOps;
use crate::model::{AccountId, ModifyAction, ModifyKind, SnoozeChange, ThreadId};
use crate::writer::Ctx;

/// A snoozed thread that gains `INBOX` from outside this app loses its snooze.
/// Judged on net labels, skipping messages with an op still pending.
pub fn apply_rule(
    ctx: &mut Ctx<'_>,
    threads: &[ThreadId],
    pending: &PendingOps,
) -> anyhow::Result<()> {
    for &t in threads {
        let snoozed: bool = ctx
            .tx
            .query_row(
                "SELECT snooze_until IS NOT NULL FROM threads WHERE id = ?1",
                [t],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !snoozed {
            continue;
        }
        let msgs = crate::threads::messages(ctx.tx, t)?;
        let gained = msgs
            .iter()
            .any(|m| m.live() && m.has("INBOX") && !pending.touches(&m.gmail_id));
        if gained {
            ctx.tx
                .execute("UPDATE threads SET snooze_until = NULL WHERE id = ?1", [t])?;
            crate::threads::refresh(ctx, t)?;
        }
    }
    Ok(())
}

/// Wakes every due snooze: adds `INBOX` and clears `snooze_until` locally at
/// once, and queues the label change.
pub fn wake_due(e: &EngineRef) {
    e.writer.sync(|ctx| {
        let due: Vec<(AccountId, ThreadId)> = {
            let mut stmt = ctx.tx.prepare(
                "SELECT account_id, id FROM threads WHERE snooze_until IS NOT NULL AND snooze_until <= ?1",
            )?;
            stmt.query_map([ctx.now], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?
        };
        for (account, thread) in due {
            crate::mail::modify(
                ctx,
                0,
                &ModifyAction {
                    account,
                    kind: ModifyKind::ReturnToInbox,
                    threads: vec![thread],
                    add: vec!["INBOX".into()],
                    remove: vec![],
                    snooze: Some(SnoozeChange::Clear),
                    undoable: false,
                },
            )?;
        }
        ctx.effect(crate::writer::Effect::Badge);
        Ok(())
    });
}

/// Wakes snoozes as they come due while the app runs.
pub async fn run(e: EngineRef) {
    loop {
        let next: Option<i64> = e
            .reads
            .with(|c| {
                Ok(c.query_row(
                    "SELECT min(snooze_until) FROM threads WHERE snooze_until IS NOT NULL",
                    [],
                    |r| r.get(0),
                )?)
            })
            .ok()
            .flatten();
        let now = crate::clock::now_ms();
        let ms = next.map_or(60_000, |n| (n - now).clamp(250, 60_000));
        // A new snooze re-plans the timer.
        e.snooze_kick
            .wait(Duration::from_millis(ms.unsigned_abs()))
            .await;
        if next.is_some_and(|n| n <= crate::clock::now_ms()) {
            wake_due(&e);
        }
    }
}
