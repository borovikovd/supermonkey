//! The fetch queue's workers, and the read of a thread the user opens (SPEC
//! Sync).

use std::collections::HashSet;
use std::sync::Arc;

use ub_domain::{GmailMessageId, GmailThreadId, ThreadId};
use ub_gmail::{Backoff, Format, GmailError, HistoryId, Outcome, Priority, ThreadQuery};
use ub_store::Priority as WritePriority;

use super::contacts::backfill_page;
use super::ctx::{AccountCtx, AfterError};
use super::ingest::{drop_thread, store_thread};
use super::queue::{Listing, Slot, Work};
use super::read::read_thread;
use crate::ops::LocalChanges;

/// Fetch workers per account. The account's scheduler caps how many of
/// their requests run at once.
pub(crate) const FETCHERS: usize = 4;
/// Threads per connect-sync `threads.list` page.
const LIST_PAGE: u32 = 500;

/// One fetch worker: takes the queue's next item and runs it until it is
/// done, gone, or the account stops.
pub(crate) async fn run(ctx: Arc<AccountCtx>) {
    loop {
        let (slot, work) = ctx.queue.next(ctx.clock()).await;
        if retrying(&ctx, || run_one(&ctx, slot, &work)).await == Ended::SignedOut {
            return;
        }
    }
}

/// Reads a thread the user opens, or a neighbor prefetched for it, at
/// priority 2, unless every body it has is stored.
pub(crate) async fn fetch_opened(ctx: Arc<AccountCtx>, thread: ThreadId) {
    let account = ctx.account;
    let gmail_id = ctx
        .read(move |r| {
            let Some(row) = r.threads().row(thread)? else {
                return Ok(None);
            };
            let needs = r.messages().thread_needs_bodies(account, &row.gmail_id)?;
            Ok(needs.then_some(row.gmail_id))
        })
        .await;
    let gmail_id = match gmail_id {
        Ok(Some(id)) => id,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(error = %e, "couldn't read an opened thread");
            return;
        }
    };
    retrying(&ctx, || async {
        fetch_thread(&ctx, &gmail_id, Format::Full, Priority::P2).await
    })
    .await;
}

/// How a retried call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ended {
    /// It succeeded, or its item is gone.
    Done,
    /// The account signed out: the worker is stopping.
    SignedOut,
}

/// Runs `call` until it succeeds or its outcome says to stop trying, backing
/// off as SPEC Gmail errors says.
pub(crate) async fn retrying<F, Fut>(ctx: &AccountCtx, mut call: F) -> Ended
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), GmailError>>,
{
    let mut backoff = Backoff::new();
    loop {
        match call().await {
            Ok(()) => {
                ctx.health.succeeded();
                return Ended::Done;
            }
            Err(err) => match ctx.after_error(&err, &mut backoff).await {
                AfterError::Retry => {}
                AfterError::Drop => return Ended::Done,
                AfterError::Stop => return Ended::SignedOut,
            },
        }
    }
}

async fn run_one(ctx: &AccountCtx, slot: Slot, work: &Work) -> Result<(), GmailError> {
    match work {
        Work::List {
            listing,
            page,
            meta,
            body,
        } => {
            let query = ThreadQuery {
                q: match listing {
                    Listing::Query(q) => Some((*q).to_owned()),
                    Listing::Label(_) => None,
                },
                label_ids: match listing {
                    Listing::Label(label) => vec![label.clone()],
                    Listing::Query(_) => Vec::new(),
                },
                page_token: page.clone(),
                max_results: Some(LIST_PAGE),
                include_spam_trash: false,
            };
            let listed = ctx.gmail.list_threads(slot.priority, &query).await?;
            for thread in listed.threads {
                ctx.queue.push(
                    *meta,
                    Work::Meta {
                        thread: thread.id,
                        history: thread.history_id,
                        body: *body,
                    },
                );
            }
            if let Some(next) = listed.next_page_token {
                ctx.queue.push(
                    slot,
                    Work::List {
                        listing: listing.clone(),
                        page: Some(next),
                        meta: *meta,
                        body: *body,
                    },
                );
            }
            Ok(())
        }
        Work::Meta {
            thread,
            history,
            body,
        } => {
            if !is_current(ctx, thread, *history).await {
                fetch_thread(ctx, thread, Format::Metadata, slot.priority).await?;
            }
            if let Some(body) = body {
                ctx.queue.push(
                    *body,
                    Work::Body {
                        thread: thread.clone(),
                    },
                );
            }
            Ok(())
        }
        Work::Body { thread } => {
            if needs_bodies(ctx, thread).await {
                fetch_thread(ctx, thread, Format::Full, slot.priority).await?;
            }
            Ok(())
        }
        Work::Backfill { page } => backfill_page(ctx, page.clone()).await,
    }
}

/// Reads a thread and stores it. A 404 means Gmail no longer has it: it
/// goes, with no toast (SPEC Gmail errors).
pub(crate) async fn fetch_thread(
    ctx: &AccountCtx,
    thread: &GmailThreadId,
    format: Format,
    priority: Priority,
) -> Result<(), GmailError> {
    let account = ctx.account;
    let stored = if format == Format::Full {
        let id = thread.clone();
        ctx.read(move |r| r.messages().gmail_ids_with_body(account, &id))
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "couldn't read which bodies are stored");
                Vec::new()
            })
    } else {
        Vec::new()
    };
    let stored: HashSet<GmailMessageId> = stored.into_iter().collect();
    let ticket = ctx.lane.landed.read();
    match read_thread(ctx, thread, format, priority, &stored).await {
        Ok(read) => {
            let stored = ctx
                .writer()
                .call(WritePriority::Sync, move |tx, state| {
                    let local = LocalChanges::load(tx.read(), account, ticket.landed())?;
                    store_thread(tx, state, account, read, &local)
                })
                .await;
            if let Err(e) = stored {
                tracing::warn!(account = account.0, error = %e, "couldn't store a thread");
            }
            Ok(())
        }
        Err(e) if *e.outcome() == Outcome::NotFound => {
            let gone = thread.clone();
            let dropped = ctx
                .writer()
                .call(WritePriority::Sync, move |tx, _| {
                    drop_thread(tx, account, &gone)
                })
                .await;
            if let Err(e) = dropped {
                tracing::warn!(account = account.0, error = %e, "couldn't drop a thread Gmail no longer has");
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Whether the cached thread is at Gmail's `history` already, so a metadata
/// read would change nothing.
pub(crate) async fn is_current(
    ctx: &AccountCtx,
    thread: &GmailThreadId,
    history: Option<HistoryId>,
) -> bool {
    let Some(history) = history else {
        return false;
    };
    let (account, id) = (ctx.account, thread.clone());
    let stored = ctx
        .read(move |r| {
            let Some(t) = r.threads().by_gmail_id(account, &id)? else {
                return Ok(None);
            };
            Ok(r.threads().row(t)?.and_then(|row| row.history_id))
        })
        .await;
    match stored {
        Ok(stored) => stored == Some(history),
        Err(e) => {
            tracing::warn!(error = %e, "couldn't read a thread's history id");
            false
        }
    }
}

/// Whether the thread is not stored, or a message of it lacks its body.
pub(crate) async fn needs_bodies(ctx: &AccountCtx, thread: &GmailThreadId) -> bool {
    let (account, id) = (ctx.account, thread.clone());
    ctx.read(move |r| r.messages().thread_needs_bodies(account, &id))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "couldn't read whether a thread has its bodies");
            true
        })
}
