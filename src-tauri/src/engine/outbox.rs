//! The outbox runner (SPEC Send and undo).
//!
//! When `send_after` passes, the row becomes sending and Rust downloads any
//! missing parts, builds the MIME, writes the Gmail draft, and calls
//! `drafts.send`. Gmail unreachable before `drafts.send` puts the row back to
//! waiting. No clear answer from `drafts.send` keeps it sending until
//! `drafts.get` settles it: 404 means sent.

use std::sync::Arc;

use rusqlite::{OptionalExtension, params};
use ub_gmail::{Format, GmailError, Outcome, Priority, cost};

use super::{AccountRt, EngineRef};
use crate::model::AccountId;

#[derive(Debug, Clone)]
struct Row {
    id: i64,
    draft: String,
    state: String,
    send_after: i64,
}

fn rows(e: &EngineRef, account: AccountId) -> Vec<Row> {
    e.reads
        .with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, draft_id, state, send_after FROM outbox
                 WHERE account_id = ?1 AND state != 'sent' ORDER BY send_after, id",
            )?;
            let rows = stmt.query_map([account], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    draft: r.get(1)?,
                    state: r.get(2)?,
                    send_after: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap_or_default()
}

pub fn on_launch(e: &EngineRef) {
    e.writer.sync(|ctx| {
        crate::outbox::on_launch(ctx)?;
        crate::drafts::on_launch(ctx)
    });
}

pub async fn run(e: EngineRef, rt: Arc<AccountRt>) {
    let mut attempt = 0u32;
    loop {
        if rt.is_stopped() {
            return;
        }
        let now = crate::db::now_ms();
        let mut next_due: Option<i64> = None;
        let mut failed: Option<GmailError> = None;
        for row in rows(&e, rt.id) {
            let r = match row.state.as_str() {
                "sending" => settle(&e, &rt, &row).await,
                _ if row.send_after <= now => send(&e, &rt, &row).await,
                _ => {
                    next_due =
                        Some(next_due.map_or(row.send_after, |d: i64| d.min(row.send_after)));
                    Ok(())
                }
            };
            e.note(&rt, r.as_ref().map(drop));
            if let Err(err) = r {
                failed = Some(err);
                break;
            }
        }
        let wait_ms = match &failed {
            Some(err) if matches!(err.outcome, Outcome::InvalidGrant) => return,
            Some(err) if matches!(err.outcome, Outcome::Offline) => 10_000,
            Some(err) => {
                let d = ub_gmail::backoff(attempt, err.retry_after());
                attempt = attempt.saturating_add(1);
                d.as_millis() as u64
            }
            None => {
                attempt = 0;
                next_due.map_or(30_000, |d| {
                    (d - crate::db::now_ms()).clamp(50, 30_000) as u64
                })
            }
        };
        super::wait(&rt.outbox_kick, wait_ms).await;
    }
}

async fn set_state(
    e: &EngineRef,
    account: AccountId,
    outbox: i64,
    state: &'static str,
    delayed: bool,
) -> bool {
    e.writer
        .call(move |ctx| {
            let n = ctx.tx.execute(
                "UPDATE outbox SET state = ?2, delayed = delayed OR ?3 WHERE id = ?1 AND state != 'sent'",
                params![outbox, state, delayed],
            )?;
            if n > 0 {
                ctx.touch_outbox(account, outbox)?;
                touch_row_thread(ctx, account, outbox)?;
            }
            Ok(n > 0)
        })
        .await
        .unwrap_or(false)
}

fn touch_row_thread(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    outbox: i64,
) -> anyhow::Result<()> {
    let t: Option<i64> = ctx
        .tx
        .query_row(
            "SELECT thread_id FROM outbox WHERE id = ?1",
            [outbox],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if let Some(t) = t
        && ctx
            .tx
            .query_row("SELECT 1 FROM threads WHERE id = ?1", [t], |_| Ok(()))
            .is_ok()
    {
        ctx.touch_thread(account, t)?;
    }
    Ok(())
}

async fn send(e: &EngineRef, rt: &Arc<AccountRt>, row: &Row) -> Result<(), GmailError> {
    let late = crate::db::now_ms() > row.send_after + 5_000;
    // Undone meanwhile: the row is gone.
    if !set_state(e, rt.id, row.id, "sending", late).await {
        return Ok(());
    }
    let Some(d) = super::drafts::load(e, &row.draft) else {
        return Ok(());
    };
    // 1–3: parts, MIME, and the Gmail draft. Failing here, Gmail never got a send.
    let draft = match super::drafts::write(e, rt, &d).await {
        Ok(g) => g,
        Err(err) => {
            return if let Outcome::Rejected { ref message, .. } = err.outcome {
                fail(e, rt.id, row, message.clone()).await;
                Ok(())
            } else {
                set_state(e, rt.id, row.id, "waiting", true).await;
                Err(err)
            };
        }
    };
    super::drafts::store_ids(e, &row.draft, &draft).await;
    record_gmail_draft(e, row.id, &draft.id).await;
    // 4: drafts.send.
    send_draft(e, rt, row, &draft.id, draft.message.thread_id.clone()).await
}

async fn record_gmail_draft(e: &EngineRef, outbox: i64, gid: &str) {
    let gid = gid.to_string();
    let _ = e
        .writer
        .call(move |ctx| {
            ctx.tx.execute(
                "UPDATE outbox SET gmail_draft_id = ?2 WHERE id = ?1",
                params![outbox, gid],
            )?;
            Ok(())
        })
        .await;
}

async fn send_draft(
    e: &EngineRef,
    rt: &Arc<AccountRt>,
    row: &Row,
    gmail_draft: &str,
    gmail_thread: String,
) -> Result<(), GmailError> {
    // A held undo before drafts.send: do not send.
    if undo_requested(e, row.id) {
        not_sent(e, rt.id, row).await;
        return Ok(());
    }
    match rt
        .sched
        .run(
            Priority::P1,
            cost::DRAFTS_SEND,
            rt.gmail.send_draft(gmail_draft),
        )
        .await
    {
        Ok(m) => {
            sent(e, rt, row, &m.id, &m.thread_id).await;
            Ok(())
        }
        // The request never left: back to waiting.
        Err(err) if matches!(err.outcome, Outcome::Offline) => {
            set_state(e, rt.id, row.id, "waiting", true).await;
            Err(err)
        }
        // No clear answer: stay sending; drafts.get settles it.
        Err(err)
            if matches!(
                err.outcome,
                Outcome::NoResponse | Outcome::ServerError { .. }
            ) =>
        {
            let _ = gmail_thread;
            Err(err)
        }
        // The draft is gone: the row stays sending and the next pass settles it.
        Err(err) if matches!(err.outcome, Outcome::NotFound) => {
            rt.outbox_kick.notify_one();
            Ok(())
        }
        Err(err) => match err.outcome {
            Outcome::Rejected { message, .. } => {
                fail(e, rt.id, row, message).await;
                Ok(())
            }
            _ => Err(err),
        },
    }
}

fn undo_requested(e: &EngineRef, outbox: i64) -> bool {
    e.reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT undo_requested FROM outbox WHERE id = ?1",
                [outbox],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
        })
        .unwrap_or(false)
}

/// A sending row with no clear answer: `drafts.get` 404 means it was sent.
async fn settle(e: &EngineRef, rt: &Arc<AccountRt>, row: &Row) -> Result<(), GmailError> {
    let gid: Option<String> = e
        .reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT gmail_draft_id FROM outbox WHERE id = ?1",
                [row.id],
                |r| r.get(0),
            )?)
        })
        .ok()
        .flatten();
    let Some(gid) = gid else {
        // Never reached Gmail: the next pass sends it from the start.
        set_state(e, rt.id, row.id, "waiting", true).await;
        rt.outbox_kick.notify_one();
        return Ok(());
    };
    match rt
        .sched
        .run(
            Priority::P1,
            cost::drafts_get(Format::Minimal),
            rt.gmail.get_draft(&gid, Format::Minimal),
        )
        .await
    {
        Ok(d) => send_draft(e, rt, row, &gid, d.message.thread_id).await,
        Err(err) if matches!(err.outcome, Outcome::NotFound) => {
            // Sent: the message is the newest SENT message from this account on the thread.
            let thread = e
                .reads
                .with(|c| {
                    Ok(c.query_row(
                        "SELECT coalesce(t.gmail_id, d.gmail_thread_id) FROM outbox o
                         LEFT JOIN drafts d ON d.id = o.draft_id LEFT JOIN threads t ON t.id = o.thread_id
                         WHERE o.id = ?1",
                        [row.id],
                        |r| r.get::<_, Option<String>>(0),
                    )?)
                })
                .ok()
                .flatten();
            let Some(thread) = thread else {
                sent(e, rt, row, "", "").await;
                return Ok(());
            };
            let t = rt
                .sched
                .run(
                    Priority::P1,
                    cost::threads_get(Format::Minimal),
                    rt.gmail.get_thread(&thread, Format::Minimal, &[]),
                )
                .await?;
            let newest = t
                .messages
                .iter()
                .filter(|m| m.label_ids.iter().any(|l| l == "SENT"))
                .max_by_key(|m| m.internal_date.unwrap_or(0))
                .map(|m| m.id.clone())
                .unwrap_or_default();
            sent(e, rt, row, &newest, &thread).await;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// Gmail accepted the send. Store the sent message at once, so the reply
/// never leaves the thread while waiting for the next poll.
async fn sent(e: &EngineRef, rt: &Arc<AccountRt>, row: &Row, message: &str, gmail_thread: &str) {
    let account = rt.id;
    let outbox = row.id;
    let draft = row.draft.clone();
    let msg = message.to_string();
    let held = e
        .writer
        .call(move |ctx| {
            ctx.tx.execute(
                "UPDATE outbox SET state = 'sent', sent_message_id = ?2 WHERE id = ?1",
                params![outbox, msg],
            )?;
            // Gmail removed the draft as it sent.
            let copies: Vec<i64> = {
                let mut stmt = ctx
                    .tx
                    .prepare("SELECT id FROM messages WHERE account_id = ?1 AND ub_draft_id = ?2 AND is_draft = 1")?;
                stmt.query_map(params![account, draft], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            for m in copies {
                super::ingest::delete_message(ctx, m)?;
            }
            let held: bool = ctx
                .tx
                .query_row("SELECT undo_requested FROM outbox WHERE id = ?1", [outbox], |r| r.get(0))?;
            if held && !msg.is_empty() {
                // A held undo on a send that went out: trash that message.
                crate::outbox::trash_sent(ctx, account, &msg)?;
                ctx.tx.execute("UPDATE outbox SET toast_closed = 1 WHERE id = ?1", [outbox])?;
                crate::undo::drop_send_entry(ctx, account, outbox)?;
            }
            ctx.touch_outbox(account, outbox)?;
            touch_row_thread(ctx, account, outbox)?;
            crate::drafts::account_of(ctx, &draft)?;
            Ok(held)
        })
        .await
        .unwrap_or(false);
    if !gmail_thread.is_empty() {
        let stored =
            super::sync::fetch_thread(e, rt, gmail_thread, Format::Full, Priority::P1).await;
        if let Ok(Some(thread)) = stored {
            let _ = e
                .writer
                .call(move |ctx| {
                    ctx.tx.execute(
                        "UPDATE outbox SET thread_id = coalesce(thread_id, ?2) WHERE id = ?1",
                        params![outbox, thread],
                    )?;
                    touch_row_thread(ctx, account, outbox)?;
                    crate::outbox::cleanup(ctx, account)
                })
                .await;
        }
    }
    let _ = held;
}

/// A held undo resolved as not sent: the draft goes back to a composer.
async fn not_sent(e: &EngineRef, account: AccountId, row: &Row) {
    notice_and_remove(e, account, row, "send_undone", "Send undone".into()).await;
}

/// Gmail rejected the message: a toast shows the error and the draft goes back to a composer.
async fn fail(e: &EngineRef, account: AccountId, row: &Row, message: String) {
    notice_and_remove(
        e,
        account,
        row,
        "send_failed",
        format!("Couldn't send: {message}"),
    )
    .await;
}

async fn notice_and_remove(
    e: &EngineRef,
    account: AccountId,
    row: &Row,
    kind: &'static str,
    message: String,
) {
    let outbox = row.id;
    let draft = row.draft.clone();
    let _ = e
        .writer
        .call(move |ctx| {
            touch_row_thread(ctx, account, outbox)?;
            crate::undo::drop_send_entry(ctx, account, outbox)?;
            ctx.tx
                .execute("DELETE FROM outbox WHERE id = ?1", [outbox])?;
            ctx.tx.execute(
                "UPDATE drafts SET started_fresh = 0, defaults_json = NULL WHERE id = ?1",
                [&draft],
            )?;
            let thread: Option<i64> = ctx
                .tx
                .query_row(
                    "SELECT thread_id FROM drafts WHERE id = ?1",
                    [&draft],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            ctx.tx.execute(
                "INSERT INTO notices (account_id, kind, message, draft_id, thread_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![account, kind, message, draft, thread, ctx.now],
            )?;
            ctx.touch_draft(account, &draft)?;
            ctx.touch_summary(account);
            Ok(())
        })
        .await;
}
