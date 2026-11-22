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

const PAGE: u32 = 15;
const TYPING_PAUSE: Duration = Duration::from_millis(300);

pub(crate) struct GmailPage {
    pub(crate) query: GmailQuery,
    pub(crate) token: Option<PageToken>,
}

#[derive(Default)]
pub(crate) struct Searches {
    running: Arc<Runs<SubId>>,
}

impl Searches {
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
            let listed = tokio::select! {
                () = listing.run.stopped().cancelled() => None,
                listed = list_page(&ctx, &progress, &query, page.as_ref()) => Some(listed),
            };
            if let Some((hits, next)) = listed {
                listing.settle(hits, next);
            }
        });
    }

    pub(crate) fn cancel(&self, sub: &SubId) {
        self.running.stop(sub);
    }
}

struct Listing {
    run: Run<SubId>,
    to_settle: Option<(CoreWriter, SubId)>,
}

impl Listing {
    fn settle(mut self, hits: Vec<ThreadId>, next: Option<PageToken>) {
        if let Some((writer, sub)) = self.to_settle.take() {
            settle(&writer, sub, hits, next);
        }
    }
}

impl Drop for Listing {
    fn drop(&mut self) {
        if let Some((writer, sub)) = self.to_settle.take()
            && self.run.is_current()
        {
            settle(&writer, sub, Vec::new(), None);
        }
    }
}

struct Progress {
    writer: CoreWriter,
    sub: SubId,
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

async fn list_page(
    ctx: &AccountCtx,
    progress: &Progress,
    query: &GmailQuery,
    page: Option<&PageToken>,
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

async fn list_when_reachable(
    ctx: &AccountCtx,
    progress: &Progress,
    request: &ThreadQuery<'_>,
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
                    network.wait(ctx.clock(), OFFLINE_RETRY).await;
                }
            }
            answer => return answer,
        }
    }
}

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

fn thread_query<'a>(query: &'a GmailQuery, page: Option<&'a PageToken>) -> ThreadQuery<'a> {
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
            q.label_ids = std::slice::from_ref(gmail_label);
            q.include_spam_trash = *include_spam_trash;
        }
    }
    q
}

fn settle(writer: &CoreWriter, sub: SubId, hits: Vec<ThreadId>, next: Option<PageToken>) {
    writer.queue(WritePriority::Sync, "merge Gmail hits", move |_, state| {
        state.subs.merge_gmail_hits(&sub, hits, next);
        Ok(())
    });
}
