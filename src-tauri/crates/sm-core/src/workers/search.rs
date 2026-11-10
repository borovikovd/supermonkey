//! Gmail listings for list subscriptions (SPEC Search, Views): a search's or
//! a Done, All Mail, label, Trash, or Spam view's `threads.list`, a page at a
//! time. Each page's hits are stored, then merged into the subscription,
//! which stops showing `searching` and offers the next page. A listing that
//! can't reach Gmail, opened offline or cut off, waits for it while its
//! subscription shows the local results, and runs once Gmail answers again.

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use sm_domain::{AccountId, GmailThreadId, SubId, ThreadId};
use sm_gmail::{Format, GmailError, Outcome, PageToken, Priority, Thread, ThreadPage, ThreadQuery};
use sm_store::Priority as WritePriority;

use super::account::{
    AccountCtx, LATE, OFFLINE_RETRY, Work, fetch_thread, is_current, needs_bodies,
};
use super::registry::AccountWorkers;
use super::runs::{Run, Runs};
use crate::clock::sleep;
use crate::effects::GmailQuery;
use crate::writer::CoreWriter;

/// Threads per Gmail search or view page: about 610 units with their
/// metadata, which fits the quota bucket.
const PAGE: u32 = 15;
/// A typed search goes to Gmail once typing pauses this long.
const TYPING_PAUSE: Duration = Duration::from_millis(300);

/// A page of a Gmail listing to fetch.
pub(crate) struct GmailPage {
    /// What is listed.
    pub(crate) query: GmailQuery,
    /// The page after the ones already listed; `None` for the first.
    pub(crate) token: Option<PageToken>,
}

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
        request: GmailPage,
    ) {
        let GmailPage { query, token: page } = request;
        self.cancel(&sub);
        let Some(worker) = workers.get(account) else {
            settle(workers.writer(), sub, Vec::new(), None);
            return;
        };
        let listing = Listing {
            run: self.running.replace(sub.clone()),
            to_settle: Some((workers.writer().clone(), sub.clone())),
        };
        let progress = Progress {
            writer: workers.writer().clone(),
            sub,
            first: page.is_none(),
        };
        let ctx = Arc::clone(worker.ctx());
        worker.ctx().tasks.spawn("gmail listing", async move {
            let stopped = listing.run.stopped().clone();
            tokio::select! {
                () = stopped.cancelled() => {}
                (hits, next) = list_page(&ctx, &progress, query, page) => listing.settle(hits, next),
            }
        });
    }

    /// A subscription closed: its listing and the reads it queued stop.
    pub(crate) fn cancel(&self, sub: &SubId) {
        self.running.stop(sub);
    }
}

/// A listing in flight. The account's worker may stop under it (replaced by
/// a sign-in, or signed out), dropping it at its next `.await`: the
/// subscription then settles with no Gmail hits, as it does for an account
/// with no worker, instead of searching forever.
struct Listing {
    run: Run<SubId>,
    /// The subscription to settle, until it is settled.
    to_settle: Option<(CoreWriter, SubId)>,
}

impl Listing {
    /// Settles the subscription with the page's hits.
    fn settle(mut self, hits: Vec<ThreadId>, next: Option<PageToken>) {
        if let Some((writer, sub)) = self.to_settle.take() {
            settle(&writer, sub, hits, next);
        }
    }
}

impl Drop for Listing {
    fn drop(&mut self) {
        // A stopped or replaced listing leaves the subscription to what
        // stopped it: closed, or listing again.
        if let Some((writer, sub)) = self.to_settle.take()
            && self.run.is_current()
        {
            settle(&writer, sub, Vec::new(), None);
        }
    }
}

/// Tells a subscription, before its listing settles, whether it is
/// searching Gmail: not while the listing waits for Gmail, and again once
/// it asks.
struct Progress {
    writer: CoreWriter,
    sub: SubId,
    /// It lists the first page, the only one the list shows as searching.
    first: bool,
}

impl Progress {
    fn searching(&self, searching: bool) {
        if !self.first {
            return;
        }
        let sub = self.sub.clone();
        self.writer.queue(
            WritePriority::Sync,
            "show whether a listing searches Gmail",
            move |_, state| {
                state.subs.set_searching(&sub, searching);
                Ok(())
            },
        );
    }
}

/// Lists one page, reads the metadata of hits not cached at their history
/// id, and queues their bodies at priority 5. Returns the hits to merge into
/// the subscription, and the next page; a failed listing has none. While
/// Gmail can't be reached, the listing waits for it (SPEC Search).
async fn list_page(
    ctx: &AccountCtx,
    progress: &Progress,
    query: GmailQuery,
    page: Option<PageToken>,
) -> (Vec<ThreadId>, Option<PageToken>) {
    if matches!(query, GmailQuery::Search(_)) && page.is_none() {
        sleep(ctx.clock(), TYPING_PAUSE).await;
    }
    let request = thread_query(query, page);
    let (threads, next) = match list_when_reachable(ctx, progress, &request).await {
        Ok(listed) => (listed.threads, listed.next_page_token),
        Err(e) => {
            tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "a Gmail listing failed");
            (Vec::new(), None)
        }
    };
    read_uncached(ctx, &threads).await;
    let hits: Vec<GmailThreadId> = threads.into_iter().map(|t| t.id).collect();
    queue_bodies(ctx, &hits).await;
    let account = ctx.account;
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
    (ids, next)
}

/// `threads.list`, once Gmail can be reached: while it can't, the listing
/// waits and its subscription shows the local results. Returns the first
/// answer that is not "no connection".
async fn list_when_reachable(
    ctx: &AccountCtx,
    progress: &Progress,
    request: &ThreadQuery,
) -> Result<ThreadPage, GmailError> {
    let network = &ctx.deps.network;
    loop {
        if !network.is_online() {
            progress.searching(false);
            network.until_online(ctx.clock(), OFFLINE_RETRY).await;
            progress.searching(true);
        }
        let listed = ctx.gmail.list_threads(Priority::P2, request).await;
        ctx.note(&listed);
        match listed {
            Err(e) if *e.outcome() == Outcome::Offline => {
                tracing::info!(
                    account = ctx.account.0,
                    "a Gmail listing can't reach Gmail; it runs when it can"
                );
                if network.is_online() {
                    // Another call reached Gmail meanwhile: try again after
                    // the usual offline wait, not at once.
                    network.wait(ctx.clock(), OFFLINE_RETRY).await;
                }
            }
            answer => return answer,
        }
    }
}

/// Reads the metadata of the listed threads not cached at their history
/// id, all at once.
async fn read_uncached(ctx: &AccountCtx, threads: &[Thread]) {
    let current = join_all(
        threads
            .iter()
            .map(|t| is_current(&ctx.deps.pool, ctx.account, &t.id, t.history_id)),
    )
    .await;
    let reads = threads
        .iter()
        .zip(current)
        .filter(|(_, current)| !current)
        .map(|(t, _)| fetch_thread(ctx, &t.id, Format::Metadata, Priority::P2));
    for read in join_all(reads).await {
        ctx.note(&read);
    }
}

/// Queues the bodies of the hits that lack them, behind everything else.
async fn queue_bodies(ctx: &AccountCtx, hits: &[GmailThreadId]) {
    for hit in hits {
        if needs_bodies(&ctx.deps.pool, ctx.account, hit.clone()).await {
            ctx.queue.push(
                LATE,
                Work::Body {
                    thread: hit.clone(),
                },
            );
        }
    }
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
fn settle(writer: &CoreWriter, sub: SubId, hits: Vec<ThreadId>, next: Option<PageToken>) {
    writer.queue(WritePriority::Sync, "merge Gmail hits", move |_, state| {
        state.subs.merge_gmail_hits(&sub, hits, next);
        Ok(())
    });
}
