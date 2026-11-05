//! Gmail listings for list subscriptions (SPEC Search, Views): a search's or
//! a Done, All Mail, label, Trash, or Spam view's `threads.list`, a page at a
//! time. Each page's hits are stored, then merged into the subscription,
//! which stops showing `searching` and offers the next page.

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use ub_domain::{AccountId, GmailThreadId, SubId};
use ub_gmail::{Format, PageToken, Priority, ThreadQuery};
use ub_store::Priority as WritePriority;

use super::account::{AccountCtx, LATE, Work, fetch_thread, is_current, needs_bodies};
use super::registry::AccountWorkers;
use super::runs::Runs;
use crate::clock::sleep;
use crate::effects::GmailQuery;
use crate::writer::CoreWriter;

/// Threads per Gmail search or view page: about 610 units with their
/// metadata, which fits the quota bucket.
const PAGE: u32 = 15;
/// A typed search goes to Gmail once typing pauses this long.
const TYPING_PAUSE: Duration = Duration::from_millis(300);

/// The listings running, by subscription.
#[derive(Default)]
pub(crate) struct Searches {
    running: Arc<Runs<SubId>>,
}

impl Searches {
    /// Lists a page for `sub`, replacing a listing it already runs. With no
    /// worker for the account (signed out), the subscription settles with
    /// no Gmail hits.
    pub(crate) fn start(
        &self,
        workers: &AccountWorkers,
        sub: SubId,
        account: AccountId,
        query: GmailQuery,
        page: Option<PageToken>,
    ) {
        self.cancel(&sub);
        let Some(worker) = workers.get(account) else {
            settle(workers.writer(), sub, Vec::new(), None);
            return;
        };
        let run = self.running.replace(sub.clone());
        let ctx = Arc::clone(worker.ctx());
        worker.ctx().tasks.spawn("gmail listing", async move {
            tokio::select! {
                () = run.stopped().cancelled() => {}
                () = list_page(&ctx, sub, query, page) => {}
            }
        });
    }

    /// A subscription closed: its listing and the reads it queued stop.
    pub(crate) fn cancel(&self, sub: &SubId) {
        self.running.stop(sub);
    }
}

/// Lists one page, reads the metadata of hits not cached at their history
/// id, queues their bodies at priority 5, and merges the hits into the
/// subscription. A failed listing settles it with none.
async fn list_page(ctx: &AccountCtx, sub: SubId, query: GmailQuery, page: Option<PageToken>) {
    if matches!(query, GmailQuery::Search(_)) && page.is_none() {
        sleep(ctx.clock(), TYPING_PAUSE).await;
    }
    let listed = ctx
        .gmail
        .list_threads(Priority::P2, &thread_query(query, page))
        .await;
    ctx.note(&listed);
    let (threads, next) = match listed {
        Ok(listed) => (listed.threads, listed.next_page_token),
        Err(e) => {
            tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "a Gmail listing failed");
            (Vec::new(), None)
        }
    };
    let current = join_all(threads.iter().map(|t| is_current(ctx, &t.id, t.history_id))).await;
    let reads = threads
        .iter()
        .zip(current)
        .filter(|(_, current)| !current)
        .map(|(t, _)| fetch_thread(ctx, &t.id, Format::Metadata, Priority::P2));
    for read in join_all(reads).await {
        ctx.note(&read);
    }
    let hits: Vec<GmailThreadId> = threads.into_iter().map(|t| t.id).collect();
    for hit in &hits {
        if needs_bodies(ctx, hit.clone()).await {
            ctx.queue.push(
                LATE,
                Work::Body {
                    thread: hit.clone(),
                },
            );
        }
    }
    let account = ctx.account;
    let writer = ctx.writer().clone();
    let ids = ctx
        .read(move |r| {
            let mut ids = Vec::with_capacity(hits.len());
            for hit in &hits {
                ids.extend(r.threads().by_gmail_id(account, hit)?);
            }
            Ok(ids)
        })
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "couldn't read a listing's hits");
            Vec::new()
        });
    settle(&writer, sub, ids, next);
}

fn thread_query(query: GmailQuery, page: Option<PageToken>) -> ThreadQuery {
    let mut q = ThreadQuery {
        page_token: page,
        max_results: Some(PAGE),
        ..ThreadQuery::default()
    };
    match query {
        GmailQuery::Search(text) | GmailQuery::View(text) => q.q = Some(text),
        GmailQuery::Label {
            gmail_label,
            include_spam_trash,
        } => {
            q.label_ids = vec![gmail_label];
            q.include_spam_trash = include_spam_trash;
        }
    }
    q
}

/// Merges a page's hits into the subscription, records its next page, and
/// ends its `searching`, with a patch even if no row changed.
fn settle(
    writer: &CoreWriter,
    sub: SubId,
    hits: Vec<ub_domain::ThreadId>,
    next: Option<PageToken>,
) {
    writer.queue(WritePriority::Sync, "merge Gmail hits", move |_, state| {
        state.subs.merge_gmail_hits(&sub, hits, next);
        Ok(())
    });
}
