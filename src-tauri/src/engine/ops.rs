//! Replays queued Gmail ops in order per account (SPEC Offline, Gmail errors).
//!
//! A queued action stays until Gmail accepts or rejects it. Only a rejection
//! rolls it back and shows an error toast; everything else retries.

use crate::LogErr as _;
use std::sync::Arc;

use rusqlite::{OptionalExtension, params};
use ub_gmail::{Format, GmailError, Outcome, Priority, cost};

use super::{AccountRt, EngineRef};
use crate::mail::ModifyOp;
use crate::model::AccountId;

#[derive(Debug, Clone)]
pub struct Op {
    pub id: i64,
    pub action: Option<i64>,
    pub kind: String,
    pub payload: String,
}

fn next_op(e: &EngineRef, account: AccountId) -> Option<Op> {
    e.reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT id, action, kind, payload FROM ops WHERE account_id = ?1 ORDER BY id LIMIT 1",
                [account],
                |r| {
                    Ok(Op {
                        id: r.get(0)?,
                        action: r.get(1)?,
                        kind: r.get(2)?,
                        payload: r.get(3)?,
                    })
                },
            )
            .optional()?)
        })
        .ok()
        .flatten()
}

pub enum Done {
    /// Remove the op.
    Finished,
    /// Store a narrowed payload and run it again (e.g. after dropping missing ids).
    Retry(String),
}

pub async fn run(e: EngineRef, rt: Arc<AccountRt>) {
    let mut attempt = 0u32;
    loop {
        if rt.is_stopped() {
            return;
        }
        let Some(op) = next_op(&e, rt.id) else {
            attempt = 0;
            super::wait(&rt.ops_kick, 30_000).await;
            continue;
        };
        let result = execute(&e, &rt, &op).await;
        e.note(&rt, result.as_ref().map(|_| ()));
        match result {
            Ok(Done::Finished) => {
                attempt = 0;
                finish(&e, rt.id, op.id).await;
            }
            Ok(Done::Retry(payload)) => {
                let id = op.id;
                e.writer
                    .call(move |ctx| {
                        ctx.tx.execute(
                            "UPDATE ops SET payload = ?2 WHERE id = ?1",
                            params![id, payload],
                        )?;
                        Ok(())
                    })
                    .await
                    .log_err("run");
            }
            Err(err) => match err.outcome {
                Outcome::Offline => super::wait(&rt.ops_kick, 10_000).await,
                Outcome::InvalidGrant => return,
                Outcome::Rejected {
                    status,
                    ref message,
                } => {
                    attempt = 0;
                    reject(&e, rt.id, &op, status, message).await;
                }
                // 404 on what an action names: it was deleted elsewhere; drop it.
                Outcome::NotFound => {
                    attempt = 0;
                    finish(&e, rt.id, op.id).await;
                }
                _ => {
                    let d = ub_gmail::backoff(attempt, err.retry_after());
                    attempt = attempt.saturating_add(1);
                    tokio::select! {
                        () = tokio::time::sleep(d) => {}
                        () = rt.ops_kick.notified() => {}
                    }
                }
            },
        }
    }
}

async fn finish(e: &EngineRef, account: AccountId, op: i64) {
    e.writer
        .call(move |ctx| {
            ctx.tx.execute("DELETE FROM ops WHERE id = ?1", [op])?;
            ctx.touch_summary(account);
            Ok(())
        })
        .await
        .log_err("finish");
}

/// Gmail rejected the op: roll back its local effect, drop its undo entry,
/// and leave a notice for the toast. Replay continues with the next op.
async fn reject(e: &EngineRef, account: AccountId, op: &Op, status: u16, message: &str) {
    let op = op.clone();
    let text = format!("Gmail rejected a change ({status}): {message}");
    e
        .writer
        .call(move |ctx| {
            match op.kind.as_str() {
                "modify" => {
                    let p: ModifyOp = serde_json::from_str(&op.payload)?;
                    crate::mail::revert(ctx, None, account, &p.threads, false)?;
                }
                "label_create" => {
                    let v: serde_json::Value = serde_json::from_str(&op.payload)?;
                    let id = v["id"].as_str().unwrap_or_default().to_string();
                    ctx.tx.execute(
                        "DELETE FROM labels WHERE account_id = ?1 AND id = ?2 AND gmail_id IS NULL",
                        params![account, id],
                    )?;
                }
                other => crate::drafts::on_rejected(ctx, account, other, &op.payload)?,
            }
            if let Some(n) = op.action {
                ctx.tx.execute("DELETE FROM undo_entries WHERE id = ?1", [n])?;
            }
            ctx.tx.execute("DELETE FROM ops WHERE id = ?1", [op.id])?;
            ctx.tx.execute(
                "INSERT INTO notices (account_id, kind, message, created_at) VALUES (?1, 'rejected', ?2, ?3)",
                params![account, text, ctx.now],
            )?;
            ctx.touch_summary(account);
            Ok(())
        })
        .await
        .log_err("reject");
}

/// Local label id → Gmail label id. A label still waiting for its create has none yet.
fn gmail_labels(e: &EngineRef, account: AccountId, local: &[String]) -> Vec<String> {
    let map = e
        .reads
        .with(|c| crate::db::queries::label_gmail_ids(c, account))
        .unwrap_or_default();
    local
        .iter()
        .map(|l| map.get(l).cloned().unwrap_or_else(|| l.clone()))
        .collect()
}

async fn execute(e: &EngineRef, rt: &Arc<AccountRt>, op: &Op) -> Result<Done, GmailError> {
    match op.kind.as_str() {
        "modify" => {
            let mut p: ModifyOp = serde_json::from_str(&op.payload).map_err(bad)?;
            if p.messages.is_empty() || (p.add.is_empty() && p.remove.is_empty()) {
                return Ok(Done::Finished);
            }
            let add = gmail_labels(e, rt.id, &p.add);
            let remove = gmail_labels(e, rt.id, &p.remove);
            for chunk in p.messages.chunks(1000) {
                let r = rt
                    .sched
                    .run(
                        Priority::P1,
                        cost::MESSAGES_BATCH_MODIFY,
                        rt.gmail.batch_modify(chunk, &add, &remove),
                    )
                    .await;
                match r {
                    Ok(()) => {}
                    // batchModify does not say which id is missing: check each.
                    Err(err) if matches!(err.outcome, Outcome::NotFound) => {
                        let mut alive = Vec::new();
                        for id in &p.messages {
                            let r = rt
                                .sched
                                .run(
                                    Priority::P1,
                                    cost::messages_get(Format::Minimal),
                                    rt.gmail.get_message(id, Format::Minimal),
                                )
                                .await;
                            match r {
                                Ok(_) => alive.push(id.clone()),
                                Err(err) if matches!(err.outcome, Outcome::NotFound) => {}
                                Err(err) => return Err(err),
                            }
                        }
                        if alive.is_empty() {
                            return Ok(Done::Finished);
                        }
                        p.messages = alive;
                        return Ok(Done::Retry(serde_json::to_string(&p).map_err(bad)?));
                    }
                    Err(err) => return Err(err),
                }
            }
            Ok(Done::Finished)
        }
        "label_create" => {
            let v: serde_json::Value = serde_json::from_str(&op.payload).map_err(bad)?;
            let local = v["id"].as_str().unwrap_or_default().to_string();
            let name = v["name"].as_str().unwrap_or_default().to_string();
            let gmail_id = match rt
                .sched
                .run(
                    Priority::P1,
                    cost::LABELS_CREATE,
                    rt.gmail.create_label(&name),
                )
                .await
            {
                Ok(l) => l.id,
                // The name already exists: refresh labels.list and use that label.
                Err(err) if matches!(err.outcome, Outcome::Conflict) => {
                    let labels = rt
                        .sched
                        .run(Priority::P1, cost::LABELS_LIST, rt.gmail.list_labels())
                        .await?;
                    match labels
                        .into_iter()
                        .find(|l| l.name.eq_ignore_ascii_case(&name))
                    {
                        Some(l) => l.id,
                        None => return Err(err),
                    }
                }
                Err(err) => return Err(err),
            };
            let account = rt.id;
            e.writer
                .call(move |ctx| map_label(ctx, account, &local, &gmail_id))
                .await
                .log_err("execute");
            Ok(Done::Finished)
        }
        other => super::drafts::execute_op(e, rt, other, &op.payload).await,
    }
}

/// Gives a locally created label its Gmail id. If a refresh already stored the
/// Gmail label as its own row, that row folds into the local one.
fn map_label(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    local: &str,
    gmail_id: &str,
) -> anyhow::Result<()> {
    let dup: Option<String> = ctx
        .tx
        .query_row(
            "SELECT id FROM labels WHERE account_id = ?1 AND gmail_id = ?2 AND id != ?3",
            params![account, gmail_id, local],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(dup) = dup {
        ctx.tx.execute(
            "DELETE FROM labels WHERE account_id = ?1 AND id = ?2",
            params![account, dup],
        )?;
        let from = format!("\"{dup}\"");
        let to = format!("\"{local}\"");
        ctx.tx.execute(
            "UPDATE messages SET label_ids = replace(label_ids, ?2, ?3) WHERE account_id = ?1 AND label_ids LIKE '%' || ?2 || '%'",
            params![account, from, to],
        )?;
        ctx.tx.execute(
            "UPDATE thread_labels SET label_id = ?2 WHERE label_id = ?1 AND thread_id IN (SELECT id FROM threads WHERE account_id = ?3)",
            params![dup, local, account],
        )?;
    }
    ctx.tx.execute(
        "UPDATE labels SET gmail_id = ?3 WHERE account_id = ?1 AND id = ?2",
        params![account, local, gmail_id],
    )?;
    ctx.touch_summary(account);
    Ok(())
}

pub fn bad(e: impl std::fmt::Display) -> GmailError {
    GmailError::new(Outcome::Rejected {
        status: 0,
        message: e.to_string(),
    })
}
