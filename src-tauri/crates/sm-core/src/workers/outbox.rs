use std::sync::Arc;
use std::time::Duration;

use sm_domain::{
    AccountId, GmailDraftId, GmailMessageId, GmailThreadId, Millis, OutboxState, SystemLabel,
};
use sm_gmail::{Backoff, Format, Outcome, Priority};
use sm_store::{Outbox, Priority as WritePriority};

use super::account::{AccountCtx, OFFLINE_RETRY, fetch_thread};
use super::drafts::{WriteFailure, load, write};
use crate::Error;
use crate::clock::Clock;
use crate::drafts::lifecycle::{self, GmailIds};
use crate::drafts::store_ids;
use crate::outbox::{Event, resolve};
use crate::read::read;
use sm_store::ReadPool;

const IDLE_WAIT: Duration = Duration::from_secs(30);
const MIN_WAIT: Duration = Duration::from_millis(50);
const LATE_AFTER: i64 = 5_000;

pub(crate) async fn run(ctx: Arc<AccountCtx>) {
    let mut backoff = Backoff::new();
    loop {
        let pass = pass(&ctx).await;
        let Some(wait) = next_wait(&ctx, pass, &mut backoff) else {
            return;
        };
        let network = &ctx.deps.network;
        tokio::select! {
            () = ctx.outbox.wait(ctx.clock(), wait) => {}
            () = network.wait(ctx.clock(), wait) => {}
        }
    }
}

fn next_wait(
    ctx: &AccountCtx,
    pass: Result<Option<Millis>, Error>,
    backoff: &mut Backoff,
) -> Option<Duration> {
    match pass {
        Ok(next_due) => {
            backoff.reset();
            Some(next_due.map_or(IDLE_WAIT, |due| until(ctx.clock(), due)))
        }
        Err(Error::Gmail(e)) => ctx.gmail_wait(&e, backoff, false),
        Err(e) => {
            tracing::warn!(account = ctx.account.0, error = %e, "the outbox couldn't read or write; trying again");
            Some(OFFLINE_RETRY)
        }
    }
}

fn until(clock: &dyn Clock, due: Millis) -> Duration {
    due.duration_since(clock.now()).clamp(MIN_WAIT, IDLE_WAIT)
}

async fn pass(ctx: &AccountCtx) -> Result<Option<Millis>, Error> {
    let account = ctx.account;
    let rows = ctx.read(move |r| r.outbox().unsent(account)).await?;
    let now = ctx.clock().now();
    let mut next_due: Option<Millis> = None;
    let mut failed: Option<Error> = None;
    for record in rows {
        let row = record.outbox;
        let done = advance(ctx, &row, now, &mut next_due).await;
        go_on(account, &row, done, &mut failed)?;
    }
    failed.map_or(Ok(next_due), Err)
}

async fn advance(
    ctx: &AccountCtx,
    row: &Outbox,
    now: Millis,
    next_due: &mut Option<Millis>,
) -> Result<(), Error> {
    if row.state == OutboxState::Sending {
        settle(ctx, row).await
    } else if row.send_after <= now {
        send(ctx, row).await
    } else {
        *next_due = Some(next_due.map_or(row.send_after, |d| d.min(row.send_after)));
        Ok(())
    }
}

fn go_on(
    account: AccountId,
    row: &Outbox,
    done: Result<(), Error>,
    failed: &mut Option<Error>,
) -> Result<(), Error> {
    match done {
        Err(Error::Gmail(e)) if *e.outcome() == Outcome::InvalidGrant => Err(Error::Gmail(e)),
        Err(e) => {
            tracing::info!(account = account.0, outbox = %row.id, error = ?e, "a send couldn't go on; trying the next");
            failed.get_or_insert(e);
            Ok(())
        }
        Ok(()) => Ok(()),
    }
}

async fn step(ctx: &AccountCtx, row: &Outbox, event: Event) -> Result<bool, Error> {
    let id = row.id;
    Ok(ctx
        .writer()
        .call(WritePriority::Sync, move |tx, state| {
            resolve(tx, state, id, event)
        })
        .await?)
}

async fn current(pool: &Arc<ReadPool>, row: &Outbox) -> Result<Option<Outbox>, Error> {
    let id = row.id;
    Ok(read(pool, move |r| r.outbox().get(id))
        .await?
        .map(|r| r.outbox))
}

async fn send(ctx: &AccountCtx, row: &Outbox) -> Result<(), Error> {
    let Some(_writing) = ctx.lane.writing.begin(row.draft.clone()) else {
        return Ok(());
    };
    let late = ctx.clock().now() > row.send_after.plus(LATE_AFTER);
    if !step(ctx, row, Event::Start { late }).await? {
        return Ok(());
    }
    let Some(record) = load(&ctx.deps.pool, row.draft.clone()).await? else {
        return Ok(());
    };
    let written = match write(ctx, record.draft).await {
        Ok(Some(written)) => written,
        Ok(None) => return Ok(()),
        Err(failure) => return write_failed(ctx, row, failure).await,
    };
    let (id, local) = (row.id, row.draft.clone());
    let ids = GmailIds::of(written);
    let (gmail, to_send) = (ids.draft.clone(), ids.draft.clone());
    let applied = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, state| {
            store_ids(tx, &local, ids)?;
            resolve(tx, state, id, Event::Written(gmail))
        })
        .await?;
    if !applied {
        return Ok(());
    }
    send_draft(ctx, row, &to_send).await
}

async fn write_failed(ctx: &AccountCtx, row: &Outbox, failure: WriteFailure) -> Result<(), Error> {
    match retryable(failure) {
        Err(message) => {
            step(ctx, row, Event::Rejected(message)).await?;
            Ok(())
        }
        Ok(failure) => {
            step(ctx, row, Event::Unreached).await?;
            Err(failure)
        }
    }
}

fn retryable(failure: WriteFailure) -> Result<Error, String> {
    match failure {
        WriteFailure::Gmail(e) => match e.outcome() {
            Outcome::Rejected { message, .. } => Err(message.clone()),
            Outcome::Offline
            | Outcome::NoResponse
            | Outcome::Unauthorized
            | Outcome::InvalidGrant
            | Outcome::RateLimited { .. }
            | Outcome::ServerError { .. }
            | Outcome::NotFound
            | Outcome::Conflict => Ok(Error::Gmail(e)),
        },
        WriteFailure::Local(message) => Err(message),
        WriteFailure::Store(e) => Ok(e),
    }
}

async fn send_draft(ctx: &AccountCtx, row: &Outbox, gmail: &GmailDraftId) -> Result<(), Error> {
    let Some(now) = current(&ctx.deps.pool, row).await? else {
        return Ok(());
    };
    if now.undo_requested {
        step(ctx, row, Event::Held).await?;
        return Ok(());
    }
    match ctx.gmail.send_draft(Priority::P1, gmail).await {
        Ok(message) => sent(ctx, row, Some(message.id), Some(message.thread_id)).await,
        Err(e) => match e.outcome() {
            Outcome::Offline | Outcome::InvalidGrant => {
                step(ctx, row, Event::Unreached).await?;
                Err(e.into())
            }
            Outcome::NotFound => {
                ctx.outbox.kick();
                Ok(())
            }
            Outcome::Rejected { message, .. } => {
                step(ctx, row, Event::Rejected(message.clone())).await?;
                Ok(())
            }
            Outcome::NoResponse
            | Outcome::Unauthorized
            | Outcome::RateLimited { .. }
            | Outcome::ServerError { .. }
            | Outcome::Conflict => Err(e.into()),
        },
    }
}

async fn settle(ctx: &AccountCtx, row: &Outbox) -> Result<(), Error> {
    let Some(gmail) = current(&ctx.deps.pool, row)
        .await?
        .and_then(|r| r.gmail_draft_id)
    else {
        step(ctx, row, Event::Unreached).await?;
        ctx.outbox.kick();
        return Ok(());
    };
    match ctx
        .gmail
        .get_draft(Priority::P1, &gmail, Format::Minimal)
        .await
    {
        Ok(_) => send_draft(ctx, row, &gmail).await,
        Err(e) if *e.outcome() == Outcome::NotFound => {
            let id = row.id;
            let thread = ctx.read(move |r| r.outbox().gmail_thread(id)).await?;
            let Some(thread) = thread else {
                return sent(ctx, row, None, None).await;
            };
            let written = load(&ctx.deps.pool, row.draft.clone())
                .await?
                .and_then(|r| lifecycle::gmail_message(&r.draft).cloned());
            let message = sent_message(ctx, &thread, written.as_ref(), row.send_after).await?;
            sent(ctx, row, message, Some(thread)).await
        }
        Err(e) => Err(e.into()),
    }
}

async fn sent_message(
    ctx: &AccountCtx,
    thread: &GmailThreadId,
    written: Option<&GmailMessageId>,
    since: Millis,
) -> Result<Option<GmailMessageId>, Error> {
    let read = match ctx
        .gmail
        .get_thread(Priority::P1, thread, Format::Minimal, &[])
        .await
    {
        Ok(read) => read,
        Err(e) if *e.outcome() == Outcome::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let sent = SystemLabel::Sent.label_id();
    let mut sent = read
        .messages
        .into_iter()
        .filter(|m| m.label_ids.contains(&sent))
        .collect::<Vec<_>>();
    if let Some(at) = sent.iter().position(|m| Some(&m.id) == written) {
        return Ok(Some(sent.swap_remove(at).id));
    }
    sent.retain(|m| m.internal_date.is_some_and(|d| d >= since));
    Ok(sent
        .into_iter()
        .min_by_key(|m| m.internal_date)
        .map(|m| m.id))
}

async fn sent(
    ctx: &AccountCtx,
    row: &Outbox,
    message: Option<GmailMessageId>,
    thread: Option<GmailThreadId>,
) -> Result<(), Error> {
    if !step(ctx, row, Event::Accepted(message)).await? {
        return Ok(());
    }
    let Some(thread) = thread else {
        return Ok(());
    };
    if !fetched(ctx, &thread).await {
        return Ok(());
    }
    let account = ctx.account;
    let stored = ctx
        .read(move |r| r.threads().by_gmail_id(account, &thread))
        .await?;
    if let Some(stored) = stored {
        step(ctx, row, Event::Stored(stored)).await?;
    }
    Ok(())
}

async fn fetched(ctx: &AccountCtx, thread: &GmailThreadId) -> bool {
    let Err(e) = fetch_thread(ctx, thread, Format::Full, Priority::P1).await else {
        return true;
    };
    tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "couldn't fetch a sent message yet");
    false
}
