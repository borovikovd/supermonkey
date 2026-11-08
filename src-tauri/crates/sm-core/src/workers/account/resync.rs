//! The full resync (SPEC Sync): `history.list` said 404, so the cursor is too
//! old and the cache is reconciled with Gmail's thread list.

use std::collections::HashMap;

use sm_domain::{GmailThreadId, Rev, SystemLabel};
use sm_gmail::{Backoff, Format, GmailError, HistoryId, Outcome, PageToken, Priority, ThreadQuery};
use sm_store::{Priority as WritePriority, ThreadSyncState};

use super::ctx::{AccountCtx, OFFLINE_RETRY};
use super::ingest::drop_thread;
use super::poller::{queue_starred_and_sent, store_cursor};
use super::queue::{INBOX_BODIES, INBOX_META, LATE, Work};
use crate::workers::drafts::{Occasion, reconcile};

/// Threads per resync `threads.list` page: about 10 units per 500 threads.
const RESYNC_PAGE: u32 = 500;

/// A resync step's Gmail call, retried as background reads are: a
/// transient outcome or no connection waits and calls again, so one error
/// does not start the whole resync over. Any other outcome stops the resync,
/// which runs again from the start at the next poll.
async fn step<T, F, Fut>(ctx: &AccountCtx, mut call: F) -> Result<T, GmailError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, GmailError>>,
{
    let mut backoff = Backoff::new();
    loop {
        match call().await {
            Ok(value) => {
                ctx.health.succeeded();
                return Ok(value);
            }
            Err(e) if *e.outcome() == Outcome::Offline => {
                ctx.health.failed(&e);
                ctx.deps.network.wait(ctx.clock(), OFFLINE_RETRY).await;
            }
            Err(e) if e.is_transient() => {
                ctx.health.failed(&e);
                let wait = backoff.next_delay(&e);
                ctx.deps.network.wait(ctx.clock(), wait).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Runs the full resync. It notifies nothing.
pub(crate) async fn full_resync(
    ctx: &AccountCtx,
    drafts_due: &mut Option<Occasion>,
) -> Result<(), GmailError> {
    tracing::info!(
        account = ctx.account.0,
        "history expired; running a full resync"
    );
    // 1. A fresh cursor, stored once the resync is done: one cut short by an
    // error or a quit leaves the old cursor, so the next poll runs it again.
    let profile = step(ctx, || ctx.gmail.get_profile(Priority::P3)).await?;
    let started = ctx.read(|r| r.meta().rev()).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "couldn't read the rev; keeping every cached thread");
        Rev(0)
    });
    // 2. Every thread id with its history id, Inbox first. New and changed
    // Inbox threads are read first.
    let mut listed: HashMap<GmailThreadId, Option<HistoryId>> = HashMap::new();
    for label in [Some(SystemLabel::Inbox.label_id()), None] {
        let mut page: Option<PageToken> = None;
        loop {
            let query = ThreadQuery {
                label_ids: label.iter().cloned().collect(),
                page_token: page.take(),
                max_results: Some(RESYNC_PAGE),
                include_spam_trash: true,
                ..ThreadQuery::default()
            };
            let threads = step(ctx, || ctx.gmail.list_threads(Priority::P4, &query)).await?;
            for thread in threads.threads {
                if label.is_some() && !listed.contains_key(&thread.id) {
                    ctx.queue.push(
                        INBOX_META,
                        Work::Meta {
                            thread: thread.id.clone(),
                            history: thread.history_id,
                            body: Some(INBOX_BODIES),
                        },
                    );
                }
                listed.insert(thread.id, thread.history_id);
            }
            match threads.next_page_token {
                Some(next) => page = Some(next),
                None => break,
            }
        }
    }
    // 3 and 4.
    reconcile_cached(ctx, &listed, started).await?;
    // Of the threads not cached, those the connect sync would read.
    queue_starred_and_sent(ctx);
    // 5. Drafts. One that fails does not hold up the resync: the next poll
    // runs the reconcile again.
    match reconcile(ctx, Occasion::Sync).await {
        Ok(()) => {}
        Err(e) if *e.outcome() == Outcome::InvalidGrant => return Err(e),
        Err(e) => {
            tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "the resync's draft reconcile failed; the next poll runs it");
            *drafts_due = Some(Occasion::Sync);
        }
    }
    // 6. The snooze rule runs as each thread is stored.
    store_cursor(ctx, profile.history_id).await;
    Ok(())
}

/// Steps 3 and 4: a cached thread missing from the list goes only if its
/// row has not changed since the resync began and Gmail confirms it is gone
/// (the list is not a snapshot); one whose history id changed has its
/// metadata read again.
async fn reconcile_cached(
    ctx: &AccountCtx,
    listed: &HashMap<GmailThreadId, Option<HistoryId>>,
    started: Rev,
) -> Result<(), GmailError> {
    let account = ctx.account;
    let cached: Vec<ThreadSyncState> = ctx
        .read(move |r| r.threads().sync_states(account))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(account = account.0, error = %e, "couldn't read the cached threads");
            Vec::new()
        });
    let mut owed = Vec::new();
    for thread in cached {
        match listed.get(&thread.gmail_id) {
            None if thread.rev <= started => {
                let check = step(ctx, || {
                    ctx.gmail
                        .get_thread(Priority::P4, &thread.gmail_id, Format::Minimal, &[])
                })
                .await;
                match check {
                    Ok(_) => {}
                    Err(e) if *e.outcome() == Outcome::NotFound => {
                        let gone = thread.gmail_id;
                        let dropped = ctx
                            .writer()
                            .call(WritePriority::Sync, move |tx, _| {
                                drop_thread(tx, account, &gone)
                            })
                            .await;
                        if let Err(e) = dropped {
                            tracing::warn!(account = account.0, error = %e, "couldn't drop a thread Gmail no longer has");
                        }
                    }
                    Err(e) if *e.outcome() == Outcome::InvalidGrant => return Err(e),
                    // Gmail refuses to say: the thread stays as it is.
                    Err(e) => {
                        tracing::info!(account = account.0, outcome = %e.outcome(), "couldn't check a cached thread in the resync; keeping it");
                    }
                }
            }
            Some(history) if thread.history_id != *history => {
                owed.push(thread.id);
                ctx.queue.push(
                    LATE,
                    Work::Meta {
                        thread: thread.gmail_id,
                        history: *history,
                        body: None,
                    },
                );
            }
            _ => {}
        }
    }
    // Recorded before the cursor moves on: a quit before these reads land
    // leaves them owed, and the next launch reads them (see `queue_owed`).
    let marked = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, _| {
            tx.threads().clear_history_ids(&owed)
        })
        .await;
    if let Err(e) = marked {
        tracing::warn!(account = account.0, error = %e, "couldn't mark the threads a resync owes a read");
    }
    Ok(())
}
