//! The outbox runner (SPEC Send and undo): one task per account worker.
//!
//! When a send's `send_after` passes, it becomes sending, and the runner
//! downloads any parts not cached, builds the MIME, writes the Gmail draft,
//! and calls `drafts.send`. Gmail unreachable before `drafts.send`, or a
//! revoked grant on it, puts it back to waiting, or, with an undo held on
//! it, resolves it as not sent. No clear answer from `drafts.send` keeps it
//! sending until `drafts.get` settles it: a 404 means it was sent. Each step
//! goes through the state machine in `crate::outbox`.

use std::sync::Arc;
use std::time::Duration;

use sm_domain::{GmailDraftId, GmailMessageId, GmailThreadId, Millis, OutboxState, SystemLabel};
use sm_gmail::{Backoff, Format, GmailError, Outcome, Priority};
use sm_store::{Outbox, Priority as WritePriority, StoreError};

use super::account::{AccountCtx, OFFLINE_RETRY, fetch_thread};
use super::drafts::{WriteFailure, load, write};
use crate::Error;
use crate::drafts::lifecycle::{self, GmailIds};
use crate::drafts::store_ids;
use crate::outbox::{Event, resolve};
use crate::read::read;
use sm_store::ReadPool;

/// How long the runner waits, at most, before it looks at the outbox again.
const IDLE_WAIT: Duration = Duration::from_secs(30);
/// The shortest wait for a send's time, so a send due now never spins.
const MIN_WAIT: Duration = Duration::from_millis(50);
/// A send that starts this long after its time missed it: `Email delayed`.
const LATE_AFTER: i64 = 5_000;

/// Sends the account's due rows until the worker stops.
pub(crate) async fn run(ctx: Arc<AccountCtx>) {
    let mut backoff = Backoff::new();
    loop {
        let pass = pass(&ctx).await;
        let wait = match pass {
            Ok(next_due) => {
                backoff.reset();
                next_due.map_or(IDLE_WAIT, |due| until(&ctx, due))
            }
            Err(Failure::Gmail(e)) => {
                ctx.health.failed(&e);
                match e.outcome() {
                    Outcome::InvalidGrant => {
                        ctx.signed_out();
                        return;
                    }
                    Outcome::Offline => OFFLINE_RETRY,
                    _ => backoff.next_delay(&e),
                }
            }
            Err(Failure::Store(e)) => {
                tracing::warn!(account = ctx.account.0, error = %e, "the outbox couldn't read or write; trying again");
                OFFLINE_RETRY
            }
        };
        let network = &ctx.deps.network;
        tokio::select! {
            () = ctx.outbox.wait(ctx.clock(), wait) => {}
            () = network.wait(ctx.clock(), wait) => {}
        }
    }
}

/// Why a pass stopped before its last row.
#[derive(Debug)]
enum Failure {
    /// Gmail did not answer clearly.
    Gmail(GmailError),
    /// The database refused a read or write.
    Store(Error),
}

impl From<GmailError> for Failure {
    fn from(e: GmailError) -> Self {
        Self::Gmail(e)
    }
}

impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Self::Store(e)
    }
}

impl From<StoreError> for Failure {
    fn from(e: StoreError) -> Self {
        Self::Store(e.into())
    }
}

/// How long until `due`, within the runner's bounds.
fn until(ctx: &AccountCtx, due: Millis) -> Duration {
    let ms = u64::try_from(due.0.saturating_sub(ctx.clock().now().0)).unwrap_or(0);
    Duration::from_millis(ms).clamp(MIN_WAIT, IDLE_WAIT)
}

/// One look at the outbox: settles each sending row and sends each due
/// waiting one, oldest first. Returns when the next waiting row is due. A
/// row that fails does not hold up the rows after it: the pass goes on and
/// reports the first failure at the end, unless the account signed out.
async fn pass(ctx: &AccountCtx) -> Result<Option<Millis>, Failure> {
    let account = ctx.account;
    let rows = ctx.read(move |r| r.outbox().unsent(account)).await?;
    let now = ctx.clock().now();
    let mut next_due: Option<Millis> = None;
    let mut failed: Option<Failure> = None;
    for record in rows {
        let row = record.outbox;
        let done = if row.state == OutboxState::Sending {
            settle(ctx, &row).await
        } else if row.send_after <= now {
            send(ctx, &row).await
        } else {
            next_due = Some(next_due.map_or(row.send_after, |d| d.min(row.send_after)));
            Ok(())
        };
        match done {
            Err(Failure::Gmail(e)) if *e.outcome() == Outcome::InvalidGrant => {
                return Err(Failure::Gmail(e));
            }
            Err(e) => {
                tracing::info!(account = account.0, outbox = %row.id, error = ?e, "a send couldn't go on; trying the next");
                failed.get_or_insert(e);
            }
            Ok(()) => {}
        }
    }
    failed.map_or(Ok(next_due), Err)
}

/// Writes a step of the row's state machine. Returns whether it applied:
/// the row was where the event expects it.
async fn step(ctx: &AccountCtx, row: &Outbox, event: Event) -> Result<bool, Failure> {
    let (id, now) = (row.id, ctx.clock().now());
    Ok(ctx
        .writer()
        .call(WritePriority::Sync, move |tx, state| {
            resolve(tx, state, id, event, now)
        })
        .await?)
}

/// The row as stored now, or `None` when it is gone.
async fn current(pool: &Arc<ReadPool>, row: &Outbox) -> Result<Option<Outbox>, Failure> {
    let id = row.id;
    Ok(read(pool, move |r| r.outbox().get(id))
        .await?
        .map(|r| r.outbox))
}

/// Sends a due waiting row: steps 1 to 4 of SPEC Send and undo.
async fn send(ctx: &AccountCtx, row: &Outbox) -> Result<(), Failure> {
    // An upsert of the draft is still in flight: it wakes the outbox once it
    // lands, and this send writes after it.
    let Some(_writing) = ctx.lane.writing.begin(row.draft.clone()) else {
        return Ok(());
    };
    let late = ctx.clock().now().0 > row.send_after.0.saturating_add(LATE_AFTER);
    // Undone meanwhile (the row is gone), or already taken.
    if !step(ctx, row, Event::Start { late }).await? {
        return Ok(());
    }
    let Some(draft) = load(&ctx.deps.pool, row.draft.clone()).await? else {
        return Ok(());
    };
    // 1 to 3: parts, MIME, and the Gmail draft. Failing here, Gmail never
    // got a send.
    let written = match write(ctx, &draft).await {
        Ok(Some(written)) => written,
        // The draft went with its account while its target's body was read.
        Ok(None) => return Ok(()),
        Err(failure) => {
            let retry = match failure {
                WriteFailure::Gmail(e) => match e.outcome() {
                    Outcome::Rejected { message, .. } => Err(message.clone()),
                    _ => Ok(Failure::Gmail(e)),
                },
                WriteFailure::Local(message) => Err(message),
                WriteFailure::Store(e) => Ok(Failure::Store(e)),
            };
            return match retry {
                // It can never go through as it is.
                Err(message) => {
                    step(ctx, row, Event::Rejected(message)).await?;
                    Ok(())
                }
                // Gmail never got a send: it waits again, or a held undo
                // resolves now.
                Ok(failure) => {
                    step(ctx, row, Event::Unreached).await?;
                    Err(failure)
                }
            };
        }
    };
    // The draft keeps the Gmail ids and the send its Gmail draft together.
    let (id, now, local, stored) = (
        row.id,
        ctx.clock().now(),
        row.draft.clone(),
        written.clone(),
    );
    let applied = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, state| {
            store_ids(tx, state, &local, GmailIds::of(&stored), None, now)?;
            resolve(tx, state, id, Event::Written(stored.id.clone()), now)
        })
        .await?;
    // The row went meanwhile, with its account: nothing is sent.
    if !applied {
        return Ok(());
    }
    // 4: drafts.send.
    send_draft(ctx, row, &written.id).await
}

/// `drafts.send`, unless an undo is held on the row: then it is not sent.
async fn send_draft(ctx: &AccountCtx, row: &Outbox, gmail: &GmailDraftId) -> Result<(), Failure> {
    // Gone meanwhile, with its account: never sent.
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
            // The request never left, or Gmail refused its token and the
            // refresh found the grant revoked: Gmail never took the send,
            // so it waits again, and a held undo resolves now. The run loop
            // then signs the account out.
            Outcome::Offline | Outcome::InvalidGrant => {
                step(ctx, row, Event::Unreached).await?;
                Err(e.into())
            }
            // The draft is gone: the row stays sending, and the next pass
            // settles it.
            Outcome::NotFound => {
                ctx.outbox.kick();
                Ok(())
            }
            Outcome::Rejected { message, .. } => {
                step(ctx, row, Event::Rejected(message.clone())).await?;
                Ok(())
            }
            // No clear answer: it stays sending, and `drafts.get` settles it.
            _ => Err(e.into()),
        },
    }
}

/// A sending row with no clear answer yet: `drafts.get` finds its draft,
/// which is sent again, or Gmail no longer has it, which means it was sent.
async fn settle(ctx: &AccountCtx, row: &Outbox) -> Result<(), Failure> {
    let Some(gmail) = current(&ctx.deps.pool, row)
        .await?
        .and_then(|r| r.gmail_draft_id)
    else {
        // It never reached Gmail: the next pass sends it from the start.
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
                .and_then(|d| lifecycle::gmail_message(&d).cloned());
            let message = sent_message(ctx, &thread, written.as_ref(), row.send_after).await?;
            sent(ctx, row, message, Some(thread)).await
        }
        Err(e) => Err(e.into()),
    }
}

/// The message a send that resolved as sent with no message id in hand
/// became: the one its draft was last written as, which Gmail keeps as it
/// sends, else the first this account sent on the thread no earlier than
/// the send's time. Never an older message of the thread, which an undo
/// would trash.
async fn sent_message(
    ctx: &AccountCtx,
    thread: &GmailThreadId,
    written: Option<&GmailMessageId>,
    since: Millis,
) -> Result<Option<GmailMessageId>, Failure> {
    let read = match ctx
        .gmail
        .get_thread(Priority::P1, thread, Format::Minimal, &[])
        .await
    {
        Ok(read) => read,
        // The thread went with the draft, deleted elsewhere: nothing was sent on it.
        Err(e) if *e.outcome() == Outcome::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let sent = SystemLabel::Sent.label_id();
    let mut sent = read
        .messages
        .into_iter()
        .filter(|m| m.label_ids.contains(&sent))
        .collect::<Vec<_>>();
    if let Some(m) = sent.iter().find(|m| Some(&m.id) == written) {
        return Ok(Some(m.id.clone()));
    }
    // The first sent since: this send went out before any later reply,
    // say one sent from a phone meanwhile.
    sent.retain(|m| m.internal_date.is_some_and(|d| d >= since));
    Ok(sent
        .into_iter()
        .min_by_key(|m| m.internal_date)
        .map(|m| m.id))
}

/// Gmail accepted the send. The sent message is fetched and stored at once,
/// so the reply never leaves its thread while waiting for the next poll.
async fn sent(
    ctx: &AccountCtx,
    row: &Outbox,
    message: Option<GmailMessageId>,
    thread: Option<GmailThreadId>,
) -> Result<(), Failure> {
    if !step(ctx, row, Event::Accepted(message)).await? {
        return Ok(());
    }
    let Some(thread) = thread else {
        return Ok(());
    };
    if let Err(e) = fetch_thread(ctx, &thread, Format::Full, Priority::P1).await {
        // The next poll brings it.
        tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "couldn't fetch a sent message yet");
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
