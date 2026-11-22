use std::collections::HashMap;

use sm_domain::{GmailThreadId, Rev, SystemLabel, ThreadId};
use sm_gmail::{Format, GmailError, HistoryId, Outcome, PageToken, Priority, ThreadQuery};
use sm_store::{Priority as WritePriority, ThreadSyncState};

use super::ctx::AccountCtx;
use super::fetcher::drop_gone;
use super::poller::{queue_starred_and_sent, reconcile_or_defer, store_cursor};
use super::queue::{INBOX_BODIES, INBOX_META, LATE, Work};
use crate::workers::drafts::Occasion;

const RESYNC_PAGE: u32 = 500;

pub(crate) async fn full_resync(
    ctx: &AccountCtx,
    drafts_due: &mut Option<Occasion>,
) -> Result<(), GmailError> {
    tracing::info!(
        account = ctx.account.0,
        "history expired; running a full resync"
    );
    resync(ctx, drafts_due).await
}

async fn resync(ctx: &AccountCtx, drafts_due: &mut Option<Occasion>) -> Result<(), GmailError> {
    let profile = ctx.retried(|| ctx.gmail.get_profile(Priority::P3)).await?;
    let started = ctx.read(|r| r.meta().rev()).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "couldn't read the rev; keeping every cached thread");
        Rev(0)
    });
    let listed = list_threads(ctx).await?;
    reconcile_cached(ctx, &listed, started).await?;
    queue_starred_and_sent(&ctx.queue);
    reconcile_or_defer(
        ctx,
        drafts_due,
        |e| *e.outcome() == Outcome::InvalidGrant,
        "the resync's draft reconcile failed; the next poll runs it",
    )
    .await?;
    store_cursor(ctx, profile.history_id).await;
    Ok(())
}

async fn list_threads(
    ctx: &AccountCtx,
) -> Result<HashMap<GmailThreadId, Option<HistoryId>>, GmailError> {
    let mut listed: HashMap<GmailThreadId, Option<HistoryId>> = HashMap::new();
    for label in [Some(SystemLabel::Inbox.label_id()), None] {
        let mut page: Option<PageToken> = None;
        loop {
            let query = ThreadQuery {
                label_ids: label.as_slice(),
                page_token: page.as_ref(),
                max_results: Some(RESYNC_PAGE),
                include_spam_trash: true,
                ..ThreadQuery::default()
            };
            let threads = ctx
                .retried(|| ctx.gmail.list_threads(Priority::P4, &query))
                .await?;
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
    Ok(listed)
}

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
            None if thread.rev <= started => drop_if_gone(ctx, &thread.gmail_id).await?,
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
    mark_owed(ctx, owed).await;
    Ok(())
}

async fn drop_if_gone(ctx: &AccountCtx, thread: &GmailThreadId) -> Result<(), GmailError> {
    let check = ctx
        .retried(|| {
            ctx.gmail
                .get_thread(Priority::P4, thread, Format::Minimal, &[])
        })
        .await;
    let Err(e) = check else {
        return Ok(());
    };
    match e.outcome() {
        Outcome::NotFound => drop_gone(ctx, thread).await,
        Outcome::InvalidGrant => return Err(e),
        outcome @ (Outcome::Offline
        | Outcome::NoResponse
        | Outcome::Unauthorized
        | Outcome::RateLimited { .. }
        | Outcome::ServerError { .. }
        | Outcome::Conflict
        | Outcome::Rejected { .. }) => {
            tracing::info!(account = ctx.account.0, %outcome, "couldn't check a cached thread in the resync; keeping it");
        }
    }
    Ok(())
}

async fn mark_owed(ctx: &AccountCtx, owed: Vec<ThreadId>) {
    let account = ctx.account;
    let marked = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, _| {
            tx.threads().clear_history_ids(&owed)
        })
        .await;
    if let Err(e) = marked {
        tracing::warn!(account = account.0, error = %e, "couldn't mark the threads a resync owes a read");
    }
}
