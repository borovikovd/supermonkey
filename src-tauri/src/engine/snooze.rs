//! Snooze waking and the snooze rule (SPEC Snooze).

use rusqlite::params;

use super::EngineRef;
use super::ingest::PendingOps;
use crate::model::*;
use crate::writer::Ctx;

/// A snoozed thread that gains `INBOX` from outside this app loses its snooze.
/// Judged on net labels, skipping messages with an op still pending.
pub fn apply_rule(ctx: &mut Ctx, threads: &[ThreadId], pending: &PendingOps) -> anyhow::Result<()> {
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
                ModifyAction {
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
        let now = crate::db::now_ms();
        let wait_ms = next.map(|n| (n - now).clamp(0, 60_000)).unwrap_or(60_000) as u64;
        // A new snooze re-plans the timer.
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(wait_ms.max(250)),
            e.snooze_kick.notified(),
        )
        .await;
        if next.is_some_and(|n| n <= crate::db::now_ms()) {
            wake_due(&e);
        }
    }
}

pub fn snooze_count(ctx: &Ctx, account: AccountId) -> i64 {
    ctx.tx
        .query_row(
            "SELECT count(*) FROM threads WHERE account_id = ?1 AND snooze_until IS NOT NULL",
            params![account],
            |r| r.get(0),
        )
        .unwrap_or(0)
}
