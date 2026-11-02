//! The outbox and undo send on the writer side (SPEC Send and undo).
//!
//! Send does not call Gmail. The draft goes into the outbox as waiting with
//! `send_after` ten seconds ahead; the engine's outbox runner sends it.

use rusqlite::{OptionalExtension, params};

use crate::db::from_json;
use crate::model::{AccountId, Addr, MessageId, ThreadId};
use crate::writer::{Ctx, Effect};

/// The undo window. Fixed at ten seconds, as in Superhuman; tests shorten it.
pub static DELAY_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(10_000);

pub fn send(ctx: &mut Ctx<'_>, n: i64, draft: &str) -> anyhow::Result<()> {
    let row: Option<(AccountId, Option<ThreadId>, Option<String>, String, String, String)> = ctx
        .tx
        .query_row(
            "SELECT account_id, thread_id, gmail_draft_id, to_json, cc_json, bcc_json FROM drafts WHERE id = ?1",
            [draft],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?;
    let Some((account, thread, gmail_draft, to, cc, bcc)) = row else {
        anyhow::bail!("draft {draft} is gone");
    };
    let all: Vec<Addr> = [to, cc, bcc]
        .iter()
        .flat_map(|j| from_json::<Vec<Addr>>(j))
        .collect();
    anyhow::ensure!(!all.is_empty(), "no recipients");
    anyhow::ensure!(
        all.iter().all(|a| ub_mime::is_valid_email(&a.email)),
        "a recipient is not a valid address"
    );
    // The outbox writes the final version itself.
    ctx.tx.execute(
        "DELETE FROM ops WHERE kind = 'draft_upsert' AND json_extract(payload, '$.draft') = ?1",
        [draft],
    )?;
    ctx.tx
        .execute("UPDATE drafts SET started_fresh = 0 WHERE id = ?1", [draft])?;
    ctx.tx.execute(
        "INSERT INTO outbox (account_id, draft_id, thread_id, send_after, state, gmail_draft_id)
         VALUES (?1, ?2, ?3, ?4, 'waiting', ?5)",
        params![
            account,
            draft,
            thread,
            ctx.now + DELAY_MS.load(std::sync::atomic::Ordering::Relaxed),
            gmail_draft
        ],
    )?;
    let outbox = ctx.tx.last_insert_rowid();
    crate::undo::record(
        ctx,
        n,
        account,
        "send",
        "Sent",
        serde_json::json!({ "outbox": outbox }),
    )?;
    ctx.touch_outbox(account, outbox)?;
    ctx.touch_draft(account, draft)?;
    if let Some(t) = thread {
        ctx.touch_thread(account, t)?;
    }
    ctx.effect(Effect::Outbox(account));
    Ok(())
}

fn row(
    ctx: &Ctx<'_>,
    outbox: i64,
) -> anyhow::Result<Option<(AccountId, String, Option<ThreadId>, String, Option<String>)>> {
    Ok(ctx
        .tx
        .query_row(
            "SELECT account_id, draft_id, thread_id, state, sent_message_id FROM outbox WHERE id = ?1",
            [outbox],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?)
}

/// Closing a send toast tells Rust; once sent, its entry leaves the history.
pub fn close_toast(ctx: &mut Ctx<'_>, outbox: i64) -> anyhow::Result<()> {
    let Some((account, _, _, state, _)) = row(ctx, outbox)? else {
        return Ok(());
    };
    ctx.tx
        .execute("UPDATE outbox SET toast_closed = 1 WHERE id = ?1", [outbox])?;
    if state == "sent" {
        crate::undo::drop_send_entry(ctx, account, outbox)?;
    }
    ctx.touch_outbox(account, outbox)?;
    cleanup(ctx, account)
}

/// Undo on a send: waiting cancels it and the composer reopens; sending is
/// held until the send resolves; sent trashes only that sent message.
pub fn undo_send(ctx: &mut Ctx<'_>, account: AccountId, outbox: i64) -> anyhow::Result<()> {
    let Some((acc, draft, thread, state, sent_id)) = row(ctx, outbox)? else {
        anyhow::bail!("outbox row {outbox} is gone");
    };
    anyhow::ensure!(acc == account, "outbox row belongs to another account");
    match state.as_str() {
        "waiting" => {
            ctx.tx
                .execute("DELETE FROM outbox WHERE id = ?1", [outbox])?;
            // A draft reopened by undo is never discarded as untouched.
            ctx.tx.execute(
                "UPDATE drafts SET started_fresh = 0, defaults_json = NULL WHERE id = ?1",
                [&draft],
            )?;
            ctx.touch_draft(account, &draft)?;
        }
        "sending" => {
            ctx.tx.execute(
                "UPDATE outbox SET undo_requested = 1 WHERE id = ?1",
                [outbox],
            )?;
            ctx.touch_outbox(account, outbox)?;
        }
        _ => {
            if let Some(gid) = sent_id {
                trash_sent(ctx, account, &gid)?;
            }
            ctx.tx
                .execute("DELETE FROM outbox WHERE id = ?1", [outbox])?;
            ctx.tx
                .execute("DELETE FROM drafts WHERE id = ?1", [&draft])?;
            ctx.touched.drafts.insert(draft);
            ctx.touched.draft_lists.insert(account);
        }
    }
    ctx.touch_summary(account);
    if let Some(t) = thread
        && ctx
            .tx
            .query_row("SELECT 1 FROM threads WHERE id = ?1", [t], |_| Ok(()))
            .is_ok()
    {
        ctx.touch_thread(account, t)?;
    }
    Ok(())
}

/// Trashes only the sent message, locally at once and on Gmail through the queue.
pub fn trash_sent(ctx: &mut Ctx<'_>, account: AccountId, gmail_id: &str) -> anyhow::Result<()> {
    let msg: Option<(MessageId, ThreadId, String)> = ctx
        .tx
        .query_row(
            "SELECT id, thread_id, label_ids FROM messages WHERE account_id = ?1 AND gmail_id = ?2",
            params![account, gmail_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((id, thread, labels)) = msg {
        let mut labels: Vec<String> = from_json(&labels);
        labels.retain(|l| l != "INBOX" && l != "SPAM");
        if !labels.iter().any(|l| l == "TRASH") {
            labels.push("TRASH".into());
        }
        crate::threads::set_message_labels(ctx, id, &labels)?;
        crate::threads::refresh(ctx, thread)?;
    }
    crate::queue::push(
        ctx,
        account,
        None,
        "trash_message",
        &serde_json::json!({ "message": gmail_id }),
    )?;
    Ok(())
}

/// Removes sent rows whose toast is gone and whose message is on its thread.
pub fn cleanup(ctx: &mut Ctx<'_>, account: AccountId) -> anyhow::Result<()> {
    let done: Vec<(i64, String)> = {
        let mut stmt = ctx.tx.prepare(
            "SELECT o.id, o.draft_id FROM outbox o WHERE o.account_id = ?1 AND o.state = 'sent' AND o.toast_closed = 1
               AND (o.thread_id IS NULL OR EXISTS (SELECT 1 FROM messages m WHERE m.account_id = o.account_id
                                                   AND m.gmail_id = o.sent_message_id))",
        )?;
        stmt.query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?
    };
    for (id, draft) in done {
        ctx.tx.execute("DELETE FROM outbox WHERE id = ?1", [id])?;
        ctx.tx
            .execute("DELETE FROM drafts WHERE id = ?1", [&draft])?;
        let _ = std::fs::remove_dir_all(crate::drafts::draft_dir(&draft));
        ctx.touch_summary(account);
    }
    Ok(())
}

/// On launch: drop send entries whose row is already sent, so a stray `z`
/// never trashes delivered mail; mark waiting rows that missed their time.
pub fn on_launch(ctx: &mut Ctx<'_>) -> anyhow::Result<()> {
    ctx.tx.execute(
        "DELETE FROM undo_entries WHERE kind = 'send' AND json_extract(payload, '$.outbox') IN
           (SELECT id FROM outbox WHERE state = 'sent')",
        [],
    )?;
    ctx.tx.execute(
        "UPDATE outbox SET toast_closed = 1 WHERE state = 'sent'",
        [],
    )?;
    ctx.tx.execute(
        "UPDATE outbox SET delayed = 1 WHERE state != 'sent' AND send_after < ?1",
        [ctx.now],
    )?;
    let accounts: Vec<i64> = {
        let mut stmt = ctx.tx.prepare("SELECT id FROM accounts")?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    for a in accounts {
        cleanup(ctx, a)?;
        ctx.touch_summary(a);
    }
    Ok(())
}
