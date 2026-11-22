use std::collections::HashSet;
use std::sync::Arc;

use sm_domain::{GmailMessageId, GmailThreadId, ThreadId};
use sm_gmail::{Format, GmailError, HistoryId, Outcome, Priority, ThreadQuery};
use sm_store::Priority as WritePriority;

use super::contacts::backfill_page;
use super::ctx::AccountCtx;
use super::ingest::{drop_thread, store_thread};
use super::queue::{Listing, Slot, Work};
use super::read::read_thread;
use crate::ops::LocalChanges;
use crate::read::read;
use sm_domain::AccountId;
use sm_store::ReadPool;

pub(crate) const FETCHERS: usize = 4;
const LIST_PAGE: u32 = 500;

pub(crate) async fn run(ctx: Arc<AccountCtx>) {
    loop {
        let (slot, work) = ctx.queue.next(ctx.clock()).await;
        if retrying(&ctx, || run_one(&ctx, slot, &work)).await == Ended::SignedOut {
            return;
        }
    }
}

pub(crate) async fn fetch_opened(ctx: &AccountCtx, thread: ThreadId) {
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
    retrying(ctx, || async {
        fetch_thread(ctx, &gmail_id, Format::Full, Priority::P2).await
    })
    .await;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ended {
    Done,
    SignedOut,
}

pub(crate) async fn retrying<F, Fut>(ctx: &AccountCtx, call: F) -> Ended
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), GmailError>>,
{
    let Err(err) = ctx.retried(call).await else {
        return Ended::Done;
    };
    match err.outcome() {
        Outcome::InvalidGrant => {
            ctx.signed_out();
            Ended::SignedOut
        }
        Outcome::NotFound => Ended::Done,
        outcome @ (Outcome::Offline
        | Outcome::NoResponse
        | Outcome::Unauthorized
        | Outcome::RateLimited { .. }
        | Outcome::ServerError { .. }
        | Outcome::Conflict
        | Outcome::Rejected { .. }) => {
            tracing::info!(account = ctx.account.0, %outcome, "Gmail refused a background read; dropping it");
            Ended::Done
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
                    Listing::Query(q) => Some(*q),
                    Listing::Label(_) => None,
                },
                label_ids: match listing {
                    Listing::Label(label) => std::slice::from_ref(label),
                    Listing::Query(_) => &[],
                },
                page_token: page.as_ref(),
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
            if !is_current(&ctx.deps.pool, ctx.account, thread, *history).await {
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
            if needs_bodies(&ctx.deps.pool, ctx.account, thread.clone()).await {
                fetch_thread(ctx, thread, Format::Full, slot.priority).await?;
            }
            Ok(())
        }
        Work::Backfill { page } => backfill_page(ctx, page.as_ref()).await,
    }
}

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

pub(crate) async fn is_current(
    pool: &Arc<ReadPool>,
    account: AccountId,
    thread: &GmailThreadId,
    history: Option<HistoryId>,
) -> bool {
    let Some(history) = history else {
        return false;
    };
    let id = thread.clone();
    let stored = read(pool, move |r| {
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

pub(crate) async fn needs_bodies(
    pool: &Arc<ReadPool>,
    account: AccountId,
    id: GmailThreadId,
) -> bool {
    read(pool, move |r| {
        r.messages().thread_needs_bodies(account, &id)
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, "couldn't read whether a thread has its bodies");
        true
    })
}
