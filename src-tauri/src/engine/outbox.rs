//! The outbox runner (SPEC Send and undo).
//!
//! When `send_after` passes, the row becomes sending and Rust downloads any
//! missing parts, builds the MIME, writes the Gmail draft, and calls
//! `drafts.send`. Gmail unreachable before `drafts.send` puts the row back to
//! waiting, or, with an undo held on it, resolves it as not sent. No clear
//! answer from `drafts.send` keeps it sending until
//! `drafts.get` settles it: 404 means sent.

use crate::LogErr as _;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{OptionalExtension, params};
use ub_gmail::{Format, GmailError, Outcome, Priority, cost};

use super::{AccountRt, Engine};
use crate::model::{AccountId, DraftId, MessageId, Millis, OutboxId, ThreadId};

#[derive(Debug)]
struct Row {
    id: OutboxId,
    draft: DraftId,
    /// Handed to Gmail with no clear answer yet; otherwise waiting.
    sending: bool,
    send_after: Millis,
}

fn rows(e: &Engine, account: AccountId) -> Vec<Row> {
    e.reads
        .with(|c| {
            let mut stmt = c.prepare(
                "SELECT id, draft_id, state = 'sending', send_after FROM outbox
                 WHERE account_id = ?1 AND state != 'sent' ORDER BY send_after, id",
            )?;
            let rows = stmt.query_map([account], |r| {
                Ok(Row {
                    id: r.get(0)?,
                    draft: r.get(1)?,
                    sending: r.get(2)?,
                    send_after: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap_or_default()
}

pub fn on_launch(e: &Engine) {
    e.writer.sync(|ctx| {
        crate::outbox::on_launch(ctx)?;
        crate::drafts::on_launch(ctx)
    });
}

pub async fn run(e: Arc<Engine>, rt: Arc<AccountRt>) {
    let mut attempt = 0u32;
    loop {
        if rt.is_stopped() {
            return;
        }
        let now = crate::clock::now_ms();
        let mut next_due: Option<Millis> = None;
        let mut failed: Option<GmailError> = None;
        for row in rows(&e, rt.id) {
            let r = if row.sending {
                settle(&e, &rt, &row).await
            } else if row.send_after <= now {
                send(&e, &rt, &row).await
            } else {
                next_due = Some(next_due.map_or(row.send_after, |d| d.min(row.send_after)));
                Ok(())
            };
            e.note(&rt, &r);
            if let Err(err) = r {
                failed = Some(err);
                break;
            }
        }
        let timeout = match &failed {
            Some(err) if matches!(err.outcome, Outcome::InvalidGrant) => return,
            Some(err) if matches!(err.outcome, Outcome::Offline) => Duration::from_secs(10),
            Some(err) => {
                let d = ub_gmail::backoff(attempt, err.retry_after());
                attempt = attempt.saturating_add(1);
                d
            }
            None => {
                attempt = 0;
                let ms =
                    next_due.map_or(30_000, |d| (d - crate::clock::now_ms()).clamp(50, 30_000));
                Duration::from_millis(ms.unsigned_abs())
            }
        };
        rt.outbox_kick.wait(timeout).await;
    }
}

/// Waiting becomes sending. Only a waiting row starts, so a row undone
/// meanwhile, or taken by another runner, is left alone. Starting clears the
/// row's Gmail draft id: until this pass has written the final version, the
/// Gmail draft may hold an older autosave, and `settle` must not send that.
async fn start_sending(e: &Engine, account: AccountId, outbox: OutboxId, delayed: bool) -> bool {
    e.writer
        .call(move |ctx| {
            let n = ctx.tx.execute(
                "UPDATE outbox SET state = 'sending', delayed = delayed OR ?2, gmail_draft_id = NULL
                 WHERE id = ?1 AND state = 'waiting'",
                params![outbox, delayed],
            )?;
            if n > 0 {
                ctx.touch_outbox(account, outbox)?;
                touch_row_thread(ctx, account, outbox)?;
            }
            Ok(n > 0)
        })
        .await
        .inspect_err(|err| log::warn!("start sending outbox {outbox}: {err:#}"))
        .unwrap_or(false)
}

/// Gmail never got the send. The row waits to retry, shown as delayed. If an
/// undo was held on it, the send has resolved as not sent: the draft comes
/// back now instead of after the next online pass.
async fn back_to_waiting(e: &Engine, account: AccountId, row: &Row) {
    let outbox = row.id;
    let draft = row.draft.clone();
    e.writer
        .call(move |ctx| {
            let held: Option<bool> = ctx
                .tx
                .query_row(
                    "SELECT undo_requested FROM outbox WHERE id = ?1 AND state = 'sending'",
                    [outbox],
                    |r| r.get(0),
                )
                .optional()?;
            match held {
                None => Ok(()),
                Some(true) => {
                    remove_with_notice(ctx, account, outbox, &draft, "send_undone", "Send undone")
                }
                Some(false) => {
                    ctx.tx.execute(
                        "UPDATE outbox SET state = 'waiting', delayed = 1 WHERE id = ?1",
                        [outbox],
                    )?;
                    ctx.touch_outbox(account, outbox)?;
                    touch_row_thread(ctx, account, outbox)
                }
            }
        })
        .await
        .log_err("back_to_waiting");
}

fn touch_row_thread(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    outbox: OutboxId,
) -> anyhow::Result<()> {
    let t: Option<ThreadId> = ctx
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

async fn send(e: &Engine, rt: &AccountRt, row: &Row) -> Result<(), GmailError> {
    let late = crate::clock::now_ms() > row.send_after + 5_000;
    // Undone meanwhile (the row is gone), or already taken.
    if !start_sending(e, rt.id, row.id, late).await {
        return Ok(());
    }
    let Some(d) = super::drafts::load(e, &row.draft) else {
        return Ok(());
    };
    // 1–3: parts, MIME, and the Gmail draft. Failing here, Gmail never got a send.
    let draft = match super::drafts::write(e, rt, &d).await {
        Ok(g) => g,
        Err(err) => {
            return if let Outcome::Rejected { message, .. } = &err.outcome {
                fail(e, rt.id, row, message).await;
                Ok(())
            } else {
                back_to_waiting(e, rt.id, row).await;
                Err(err)
            };
        }
    };
    super::drafts::store_ids(e, &row.draft, &draft, None).await;
    record_gmail_draft(e, row.id, draft.id.clone()).await;
    // 4: drafts.send.
    send_draft(e, rt, row, &draft.id).await
}

async fn record_gmail_draft(e: &Engine, outbox: OutboxId, gid: String) {
    e.writer
        .call(move |ctx| {
            ctx.tx.execute(
                "UPDATE outbox SET gmail_draft_id = ?2 WHERE id = ?1",
                params![outbox, gid],
            )?;
            Ok(())
        })
        .await
        .log_err("record_gmail_draft");
}

async fn send_draft(
    e: &Engine,
    rt: &AccountRt,
    row: &Row,
    gmail_draft: &str,
) -> Result<(), GmailError> {
    // A held undo before drafts.send: do not send.
    if undo_requested(e, row.id) {
        not_sent(e, rt.id, row).await;
        return Ok(());
    }
    let err = match rt
        .sched
        .run(
            Priority::P1,
            cost::DRAFTS_SEND,
            rt.gmail.send_draft(gmail_draft),
        )
        .await
    {
        Ok(m) => {
            sent(e, rt, row, m.id, &m.thread_id).await;
            return Ok(());
        }
        Err(err) => err,
    };
    match &err.outcome {
        // The request never left: back to waiting.
        Outcome::Offline => {
            back_to_waiting(e, rt.id, row).await;
            Err(err)
        }
        // The draft is gone: the row stays sending and the next pass settles it.
        Outcome::NotFound => {
            rt.outbox_kick.kick();
            Ok(())
        }
        Outcome::Rejected { message, .. } => {
            fail(e, rt.id, row, message).await;
            Ok(())
        }
        // No clear answer: stay sending; drafts.get settles it.
        _ => Err(err),
    }
}

fn undo_requested(e: &Engine, outbox: OutboxId) -> bool {
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
async fn settle(e: &Engine, rt: &AccountRt, row: &Row) -> Result<(), GmailError> {
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
        back_to_waiting(e, rt.id, row).await;
        rt.outbox_kick.kick();
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
        Ok(_) => send_draft(e, rt, row, &gid).await,
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
                sent(e, rt, row, String::new(), "").await;
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
                .into_iter()
                .filter(|m| m.label_ids.iter().any(|l| l == "SENT"))
                .max_by_key(|m| m.internal_date.unwrap_or(0))
                .map(|m| m.id)
                .unwrap_or_default();
            sent(e, rt, row, newest, &thread).await;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

/// Gmail accepted the send. Store the sent message at once, so the reply
/// never leaves the thread while waiting for the next poll.
async fn sent(e: &Engine, rt: &AccountRt, row: &Row, msg: String, gmail_thread: &str) {
    let account = rt.id;
    let outbox = row.id;
    let draft = row.draft.clone();
    e.writer
        .call(move |ctx| {
            ctx.tx.execute(
                "UPDATE outbox SET state = 'sent', sent_message_id = ?2 WHERE id = ?1",
                params![outbox, msg],
            )?;
            // Gmail removed the draft as it sent.
            let copies: Vec<MessageId> = {
                let mut stmt = ctx
                    .tx
                    .prepare("SELECT id FROM messages WHERE account_id = ?1 AND ub_draft_id = ?2 AND is_draft = 1")?;
                stmt.query_map(params![account, draft], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            for m in copies {
                super::ingest::delete_message(ctx, m)?;
            }
            let (held, toast_closed): (bool, bool) = ctx.tx.query_row(
                "SELECT undo_requested, toast_closed FROM outbox WHERE id = ?1",
                [outbox],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if held && !msg.is_empty() {
                // A held undo on a send that went out: trash that message.
                crate::outbox::trash_sent(ctx, account, &msg)?;
                ctx.tx.execute("UPDATE outbox SET toast_closed = 1 WHERE id = ?1", [outbox])?;
                crate::undo::drop_send_entry(ctx, account, outbox)?;
            } else if toast_closed {
                // Sent, and its toast is already gone: a stray `z` must not trash it.
                crate::undo::drop_send_entry(ctx, account, outbox)?;
            }
            ctx.touch_outbox(account, outbox)?;
            touch_row_thread(ctx, account, outbox)?;
            Ok(())
        })
        .await
        .log_err("sent");
    if !gmail_thread.is_empty() {
        let stored =
            super::sync::fetch_thread(e, rt, gmail_thread, Format::Full, Priority::P1).await;
        if let Ok(Some(thread)) = stored {
            e.writer
                .call(move |ctx| {
                    ctx.tx.execute(
                        "UPDATE outbox SET thread_id = coalesce(thread_id, ?2) WHERE id = ?1",
                        params![outbox, thread],
                    )?;
                    touch_row_thread(ctx, account, outbox)?;
                    crate::outbox::cleanup(ctx, account)
                })
                .await
                .log_err("sent");
        }
    }
}

/// A held undo resolved as not sent: the draft goes back to a composer.
async fn not_sent(e: &Engine, account: AccountId, row: &Row) {
    notice_and_remove(e, account, row, "send_undone", "Send undone".into()).await;
}

/// Gmail rejected the message: a toast shows the error and the draft goes back to a composer.
async fn fail(e: &Engine, account: AccountId, row: &Row, message: &str) {
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
    e: &Engine,
    account: AccountId,
    row: &Row,
    kind: &'static str,
    message: String,
) {
    let outbox = row.id;
    let draft = row.draft.clone();
    e.writer
        .call(move |ctx| remove_with_notice(ctx, account, outbox, &draft, kind, &message))
        .await
        .log_err("notice_and_remove");
}

/// Removes the row and leaves a notice that puts the draft back in a composer.
fn remove_with_notice(
    ctx: &mut crate::writer::Ctx<'_>,
    account: AccountId,
    outbox: OutboxId,
    draft: &DraftId,
    kind: &str,
    message: &str,
) -> anyhow::Result<()> {
    touch_row_thread(ctx, account, outbox)?;
    crate::undo::drop_send_entry(ctx, account, outbox)?;
    ctx.tx
        .execute("DELETE FROM outbox WHERE id = ?1", [outbox])?;
    ctx.tx.execute(
        "UPDATE drafts SET started_fresh = 0, defaults_json = NULL WHERE id = ?1",
        [draft],
    )?;
    let thread: Option<ThreadId> = ctx
        .tx
        .query_row("SELECT thread_id FROM drafts WHERE id = ?1", [draft], |r| {
            r.get(0)
        })
        .optional()?
        .flatten();
    ctx.tx.execute(
        "INSERT INTO notices (account_id, kind, message, draft_id, thread_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![account, kind, message, draft, thread, ctx.now],
    )?;
    ctx.touch_draft(account, draft)?;
    ctx.touch_summary(account);
    Ok(())
}
