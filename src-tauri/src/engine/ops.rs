//! Replays queued Gmail ops in order per account (SPEC Offline, Gmail errors).
//!
//! A queued action stays until Gmail accepts or rejects it. Only a rejection
//! rolls it back and shows an error toast; everything else retries.

use crate::LogErr as _;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use ub_gmail::{Format, GmailError, Outcome, Priority, cost};

use super::{AccountRt, Engine};
use crate::mail::ModifyOp;
use crate::model::AccountId;

#[derive(Debug)]
pub struct Op {
    pub id: i64,
    pub action: Option<i64>,
    pub kind: String,
    pub payload: String,
}

/// The payload of a `label_create` op.
#[derive(Deserialize)]
struct LabelCreate {
    /// The label's local id.
    id: String,
    name: String,
}

fn next_op(e: &Engine, account: AccountId) -> Option<Op> {
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

pub async fn run(e: Arc<Engine>, rt: Arc<AccountRt>) {
    let mut attempt = 0u32;
    loop {
        if rt.is_stopped() {
            return;
        }
        let Some(op) = next_op(&e, rt.id) else {
            attempt = 0;
            rt.ops_kick.wait(Duration::from_secs(30)).await;
            continue;
        };
        let result = execute(&e, &rt, &op).await;
        e.note(&rt, &result);
        match result {
            Ok(Done::Finished) => {
                attempt = 0;
                landed(&rt, &op);
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
                    .log_err("store narrowed op");
            }
            Err(err) => match err.outcome {
                Outcome::Offline => rt.ops_kick.wait(Duration::from_secs(10)).await,
                Outcome::InvalidGrant => return,
                Outcome::Rejected {
                    status,
                    ref message,
                } => {
                    attempt = 0;
                    reject(&e, rt.id, op, status, message).await;
                }
                // 404 on what an action names: it was deleted elsewhere; drop it.
                Outcome::NotFound => {
                    attempt = 0;
                    landed(&rt, &op);
                    finish(&e, rt.id, op.id).await;
                }
                _ => {
                    let d = ub_gmail::backoff(attempt, err.retry_after());
                    attempt = attempt.saturating_add(1);
                    rt.ops_kick.wait(d).await;
                }
            },
        }
    }
}

/// Records a modify op that left the queue, for reads still in flight.
fn landed(rt: &AccountRt, op: &Op) {
    if op.kind != "modify" {
        return;
    }
    match serde_json::from_str::<ModifyOp>(&op.payload) {
        Ok(p) => rt.landed.push(p),
        Err(err) => log::warn!("landed op {}: {err}", op.id),
    }
}

async fn finish(e: &Engine, account: AccountId, op: i64) {
    e.writer
        .call(move |ctx| {
            ctx.tx.execute("DELETE FROM ops WHERE id = ?1", [op])?;
            ctx.touch_summary(account);
            Ok(())
        })
        .await
        .log_err("finish op");
}

/// Gmail rejected the op: roll back its local effect, drop its undo entry,
/// and leave a notice for the toast. Replay continues with the next op.
async fn reject(e: &Engine, account: AccountId, op: Op, status: u16, message: &str) {
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
                    let p: LabelCreate = serde_json::from_str(&op.payload)?;
                    drop_label_uses(ctx, account, &p.id)?;
                    ctx.tx.execute(
                        "DELETE FROM labels WHERE account_id = ?1 AND id = ?2 AND gmail_id IS NULL",
                        params![account, p.id],
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
        .log_err("reject op");
}

/// A label Gmail refused to create can't be applied either: the queued
/// changes that use it are rolled back locally and dropped with their undo entries.
fn drop_label_uses(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    label: &str,
) -> anyhow::Result<()> {
    let queued: Vec<(i64, Option<i64>, String)> = {
        let mut stmt = ctx.tx.prepare(
            "SELECT id, action, payload FROM ops WHERE account_id = ?1 AND kind = 'modify' ORDER BY id",
        )?;
        stmt.query_map([account], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?
    };
    for (id, action, payload) in queued {
        let p: ModifyOp = serde_json::from_str(&payload)?;
        if !p.add.iter().chain(&p.remove).any(|l| l == label) {
            continue;
        }
        crate::mail::revert(ctx, None, account, &p.threads, false)?;
        if let Some(n) = action {
            ctx.tx
                .execute("DELETE FROM undo_entries WHERE id = ?1", [n])?;
        }
        ctx.tx.execute("DELETE FROM ops WHERE id = ?1", [id])?;
    }
    Ok(())
}

/// Local label id → Gmail label id. A label still waiting for its create has none yet.
fn gmail_labels(e: &Engine, account: AccountId, local: &[String]) -> Vec<String> {
    let map = e
        .reads
        .with(|c| crate::db::queries::label_gmail_ids(c, account))
        .unwrap_or_default();
    local
        .iter()
        .map(|l| map.get(l).cloned().unwrap_or_else(|| l.clone()))
        .collect()
}

async fn execute(e: &Engine, rt: &AccountRt, op: &Op) -> Result<Done, GmailError> {
    match op.kind.as_str() {
        "modify" => modify(e, rt, serde_json::from_str(&op.payload).map_err(bad)?).await,
        "label_create" => {
            let p: LabelCreate = serde_json::from_str(&op.payload).map_err(bad)?;
            let gmail_id = create_label(rt, &p.name).await?;
            let account = rt.id;
            e.writer
                .call(move |ctx| map_label(ctx, account, &p.id, &gmail_id))
                .await
                .log_err("map created label");
            Ok(Done::Finished)
        }
        other => super::drafts::execute_op(e, rt, other, &op.payload).await,
    }
}

/// `messages.batchModify` in chunks of 1000. A 404 narrows the op to the
/// messages that still exist.
async fn modify(e: &Engine, rt: &AccountRt, mut p: ModifyOp) -> Result<Done, GmailError> {
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
                let alive = existing_messages(rt, &p.messages).await?;
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

/// The Gmail message ids that still exist.
async fn existing_messages(rt: &AccountRt, ids: &[String]) -> Result<Vec<String>, GmailError> {
    let mut alive = Vec::new();
    for id in ids {
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
    Ok(alive)
}

/// `labels.create`, returning the Gmail label id. If the name already exists,
/// that label is used.
async fn create_label(rt: &AccountRt, name: &str) -> Result<String, GmailError> {
    let err = match rt
        .sched
        .run(
            Priority::P1,
            cost::LABELS_CREATE,
            rt.gmail.create_label(name),
        )
        .await
    {
        Ok(l) => return Ok(l.id),
        Err(err) if matches!(err.outcome, Outcome::Conflict) => err,
        Err(err) => return Err(err),
    };
    let labels = rt
        .sched
        .run(Priority::P1, cost::LABELS_LIST, rt.gmail.list_labels())
        .await?;
    labels
        .into_iter()
        .find(|l| l.name.eq_ignore_ascii_case(name))
        .map(|l| l.id)
        .ok_or(err)
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

/// A failure that is not Gmail's (a bad payload, a local file), surfaced as a
/// rejection so the op rolls back instead of retrying forever.
pub fn bad(e: impl std::fmt::Display) -> GmailError {
    GmailError::new(Outcome::Rejected {
        status: 0,
        message: e.to_string(),
    })
}
