//! The full resync (SPEC Sync): `history.list` said 404, so the cursor is too
//! old and the cache is reconciled with Gmail's thread list.

use std::collections::HashMap;

use ub_domain::{GmailThreadId, Rev, SystemLabel};
use ub_gmail::{Format, GmailError, HistoryId, Outcome, PageToken, Priority, ThreadQuery};
use ub_store::{Priority as WritePriority, ThreadSyncState};

use super::ctx::AccountCtx;
use super::ingest::drop_thread;
use super::poller::{queue_starred_and_sent, store_cursor};
use super::queue::{INBOX_BODIES, INBOX_META, LATE, Work};
use crate::workers::drafts::{Occasion, reconcile};

/// Threads per resync `threads.list` page: about 10 units per 500 threads.
const RESYNC_PAGE: u32 = 500;

/// Runs the full resync. It notifies nothing.
pub(crate) async fn full_resync(ctx: &AccountCtx) -> Result<(), GmailError> {
    tracing::info!(
        account = ctx.account.0,
        "history expired; running a full resync"
    );
    // 1. A fresh cursor first, so polling resumes at once.
    let profile = ctx.gmail.get_profile(Priority::P3).await?;
    let started = ctx.read(|r| r.meta().rev()).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "couldn't read the rev; keeping every cached thread");
        Rev(0)
    });
    store_cursor(ctx, profile.history_id).await;
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
            let threads = ctx.gmail.list_threads(Priority::P4, &query).await?;
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
    // 5. Drafts.
    reconcile(ctx, Occasion::Sync).await?;
    // 6. The snooze rule runs as each thread is stored.
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
    for thread in cached {
        match listed.get(&thread.gmail_id) {
            None if thread.rev <= started => {
                let check = ctx
                    .gmail
                    .get_thread(Priority::P4, &thread.gmail_id, Format::Minimal, &[])
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
                    Err(e) => return Err(e),
                }
            }
            Some(history) if thread.history_id != history.map(|h| h.to_string()) => {
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
    Ok(())
}
