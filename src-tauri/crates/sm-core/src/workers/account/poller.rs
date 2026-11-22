use std::collections::{BTreeSet, HashSet};
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use sm_domain::{GmailMessageId, GmailThreadId, Millis, SystemLabel};
use sm_gmail::{Format, GmailError, HistoryId, Outcome, PageToken, Priority};
use sm_store::Priority as WritePriority;

use super::ctx::{AccountCtx, OFFLINE_RETRY};
use super::history::HistoryBatch;
use super::ingest::{Polled, apply_history};
use super::profile::{refresh_labels, refresh_send_as};
use super::queue::{
    BACKFILL, FetchQueue, INBOX_BODIES, INBOX_META, LATE, Listing, OTHER_META, POLL_BODIES,
    POLL_META, Work,
};
use super::read::{ReadThread, read_thread};
use super::resync::full_resync;
use crate::notify::Arrivals;
use crate::ops::ReadTicket;
use crate::read::read;
use crate::workers::drafts::{Occasion, reconcile, stops_the_account};
use crate::workers::snooze;
use sm_domain::AccountId;
use sm_store::ReadPool;

const POLL_EVERY: Duration = Duration::from_secs(45);
const SEND_AS_EVERY: u32 = 20;
const WOKE_AFTER: Duration = Duration::from_secs(3 * 45);

pub(crate) async fn run(ctx: Arc<AccountCtx>) {
    let mut drafts_due: Option<Occasion> = None;
    if resume_or_connect(&ctx, &mut drafts_due).await.is_break() {
        return;
    }
    let mut polls = 0u32;
    let mut last = ctx.clock().now();
    loop {
        if poll_once(&ctx, &mut drafts_due).await.is_break() {
            return;
        }
        polls = polls.wrapping_add(1);
        if polls.is_multiple_of(SEND_AS_EVERY)
            && let Err(e) = refresh_send_as(&ctx).await
        {
            ctx.health.failed(&e);
        }
        wait_to_poll(&ctx).await;
        let now = ctx.clock().now();
        wake_if_slept(&ctx, last, now).await;
        last = now;
    }
}

async fn resume_or_connect(ctx: &AccountCtx, drafts_due: &mut Option<Occasion>) -> ControlFlow<()> {
    match is_connected(&ctx.deps.pool, ctx.account).await {
        Some(true) => {
            resume(ctx, drafts_due).await;
            ControlFlow::Continue(())
        }
        Some(false) => connect_until_done(ctx, drafts_due).await,
        None => ControlFlow::Break(()),
    }
}

async fn resume(ctx: &AccountCtx, drafts_due: &mut Option<Occasion>) {
    queue_inbox(&ctx.queue);
    queue_starred_and_sent(&ctx.queue);
    queue_owed(ctx).await;
    *drafts_due = Some(Occasion::Poll);
}

async fn connect_until_done(
    ctx: &AccountCtx,
    drafts_due: &mut Option<Occasion>,
) -> ControlFlow<()> {
    loop {
        match connect(ctx, drafts_due).await {
            Ok(()) => return ControlFlow::Continue(()),
            Err(e) if *e.outcome() == Outcome::InvalidGrant => {
                grant_refused(ctx, &e);
                return ControlFlow::Break(());
            }
            Err(e) => {
                connect_failed(ctx, &e);
                ctx.poll.wait(ctx.clock(), OFFLINE_RETRY).await;
            }
        }
    }
}

fn grant_refused(ctx: &AccountCtx, e: &GmailError) {
    ctx.health.failed(e);
    ctx.signed_out();
}

fn connect_failed(ctx: &AccountCtx, e: &GmailError) {
    tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "the connect sync failed; trying again");
    ctx.health.failed(e);
}

async fn poll_once(ctx: &AccountCtx, drafts_due: &mut Option<Occasion>) -> ControlFlow<()> {
    match poll(ctx, drafts_due).await {
        Ok(()) => ctx.health.succeeded(),
        Err(e) if *e.outcome() == Outcome::InvalidGrant => {
            grant_refused(ctx, &e);
            return ControlFlow::Break(());
        }
        Err(e) => poll_failed(ctx, &e),
    }
    ControlFlow::Continue(())
}

fn poll_failed(ctx: &AccountCtx, e: &GmailError) {
    tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "a poll failed");
    ctx.health.failed(e);
}

async fn wake_if_slept(ctx: &AccountCtx, last: Millis, now: Millis) {
    let gap = u64::try_from(now.0.saturating_sub(last.0)).unwrap_or(0);
    if Duration::from_millis(gap) > WOKE_AFTER {
        tracing::info!(account = ctx.account.0, "woke from sleep");
        snooze::wake_due(ctx.writer(), now).await;
        ctx.outbox.kick();
    }
}

async fn wait_to_poll(ctx: &AccountCtx) {
    let network = &ctx.deps.network;
    if network.is_online() {
        tokio::select! {
            () = ctx.poll.wait(ctx.clock(), POLL_EVERY) => return,
            () = network.until_lost(ctx.clock(), POLL_EVERY) => {}
        }
        if network.is_online() {
            return;
        }
    }
    ctx.poll.wait(ctx.clock(), OFFLINE_RETRY).await;
}

async fn is_connected(pool: &Arc<ReadPool>, account: AccountId) -> Option<bool> {
    read(pool, move |r| {
        Ok(r.accounts().get(account)?.is_some_and(|a| a.connected))
    })
    .await
    .inspect_err(|e| {
        tracing::error!(account = account.0, error = %e, "couldn't read the account; not syncing it");
    })
    .ok()
}

async fn connect(ctx: &AccountCtx, drafts_due: &mut Option<Occasion>) -> Result<(), GmailError> {
    ctx.health.syncing();
    let cursor = load_profile(ctx).await?;
    store_connect_cursor(ctx, cursor).await;
    queue_inbox(&ctx.queue);
    queue_starred_and_sent(&ctx.queue);
    reconcile_on_connect(ctx, drafts_due).await?;
    ctx.queue.push(BACKFILL, Work::Backfill { page: None });
    mark_connected(ctx).await;
    Ok(())
}

async fn load_profile(ctx: &AccountCtx) -> Result<HistoryId, GmailError> {
    let profile = ctx.gmail.get_profile(Priority::P3).await?;
    refresh_send_as(ctx).await?;
    refresh_labels(ctx).await?;
    Ok(profile.history_id)
}

async fn reconcile_on_connect(
    ctx: &AccountCtx,
    drafts_due: &mut Option<Occasion>,
) -> Result<(), GmailError> {
    match reconcile(ctx, Occasion::Sync).await {
        Ok(()) => {}
        Err(e) if stops_the_account(&e) => return Err(e),
        Err(e) => {
            tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "the connect sync's draft reconcile failed; the first poll runs it");
            *drafts_due = Some(Occasion::Sync);
        }
    }
    Ok(())
}

async fn store_connect_cursor(ctx: &AccountCtx, cursor: HistoryId) {
    let account = ctx.account;
    let stored = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, _| {
            let Some(record) = tx.read().accounts().get(account)? else {
                return Ok(());
            };
            if record.history_id.is_none() {
                tx.accounts().set_history(account, cursor)?;
            }
            Ok::<_, sm_store::StoreError>(())
        })
        .await;
    if let Err(e) = stored {
        tracing::warn!(account = account.0, error = %e, "couldn't store the connect cursor");
    }
}

async fn mark_connected(ctx: &AccountCtx) {
    let account = ctx.account;
    let connected = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, _| {
            tx.accounts().set_connected(account)
        })
        .await;
    if let Err(e) = connected {
        tracing::warn!(account = account.0, error = %e, "couldn't mark the account connected");
    }
}

async fn queue_owed(ctx: &AccountCtx) {
    let account = ctx.account;
    let owed = ctx
        .read(move |r| r.threads().owed_reads(account))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(account = account.0, error = %e, "couldn't read the threads owed a read");
            Vec::new()
        });
    for thread in owed {
        ctx.queue.push(
            LATE,
            Work::Meta {
                thread,
                history: None,
                body: None,
            },
        );
    }
}

pub(crate) fn queue_inbox(queue: &FetchQueue) {
    queue.push(
        INBOX_META,
        Work::List {
            listing: Listing::Label(SystemLabel::Inbox.label_id()),
            page: None,
            meta: INBOX_META,
            body: Some(INBOX_BODIES),
        },
    );
}

pub(crate) fn queue_starred_and_sent(queue: &FetchQueue) {
    queue.push(
        OTHER_META,
        Work::List {
            listing: Listing::Label(SystemLabel::Starred.label_id()),
            page: None,
            meta: OTHER_META,
            body: Some(LATE),
        },
    );
    queue.push(
        LATE,
        Work::List {
            listing: Listing::Query("in:sent newer_than:90d"),
            page: None,
            meta: LATE,
            body: Some(LATE),
        },
    );
}

async fn poll(ctx: &AccountCtx, drafts_due: &mut Option<Occasion>) -> Result<(), GmailError> {
    let Some(start) = history_cursor(ctx).await else {
        return Ok(());
    };
    let ticket = ctx.lane.landed.read();
    let Some((records, latest)) = history_since(ctx, start).await? else {
        return full_resync(ctx, drafts_due).await;
    };
    if records.is_empty() {
        return caught_up(ctx, drafts_due, start, latest).await;
    }
    let batch = HistoryBatch::new(records);
    let drafts_changed = batch.drafts_changed;
    let known = known_messages(ctx, batch.labels.keys().cloned().collect()).await;
    let to_read = batch.threads_to_read(|id| known.contains(id));
    let reads = read_threads(ctx, &to_read).await?;
    if let Some(deferred) = apply_poll(ctx, batch, reads, latest, ticket).await {
        queue_after_poll(&ctx.queue, to_read, deferred);
    }
    reconcile_drafts(ctx, drafts_due, drafts_changed).await
}

async fn history_cursor(ctx: &AccountCtx) -> Option<HistoryId> {
    let account = ctx.account;
    ctx.read(move |r| Ok(r.accounts().get(account)?.and_then(|a| a.history_id)))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(account = account.0, error = %e, "couldn't read the history cursor");
            None
        })
}

async fn caught_up(
    ctx: &AccountCtx,
    drafts_due: &mut Option<Occasion>,
    start: HistoryId,
    latest: HistoryId,
) -> Result<(), GmailError> {
    if latest != start {
        store_cursor(ctx, latest).await;
    }
    reconcile_drafts(ctx, drafts_due, false).await
}

async fn apply_poll(
    ctx: &AccountCtx,
    batch: HistoryBatch,
    reads: PollReads,
    cursor: HistoryId,
    ticket: ReadTicket,
) -> Option<Vec<GmailThreadId>> {
    let account = ctx.account;
    let notifier = Arc::clone(&ctx.deps.notifier);
    let PollReads { read, deferred } = reads;
    let applied = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, state| {
            let polled = Polled {
                account,
                batch: &batch,
                read,
                deferred: &deferred,
                cursor,
                ticket: &ticket,
            };
            let added = apply_history(tx, state, polled)?;
            let focused = notifier.is_focused();
            let arrivals = Arrivals {
                stored: &added,
                reported: &batch.added,
            };
            crate::notify::new_mail(tx.read(), state, account, arrivals, focused)?;
            Ok::<_, sm_store::StoreError>(deferred)
        })
        .await;
    applied
        .inspect_err(|e| {
            tracing::warn!(account = account.0, error = %e, "couldn't apply a poll's history");
        })
        .ok()
}

async fn known_messages(ctx: &AccountCtx, named: Vec<GmailMessageId>) -> BTreeSet<GmailMessageId> {
    let account = ctx.account;
    ctx.read(move |r| {
        let mut known = BTreeSet::new();
        for id in named {
            if r.messages().by_gmail_id(account, &id)?.is_some() {
                known.insert(id);
            }
        }
        Ok(known)
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(account = account.0, error = %e, "couldn't read which messages are known");
        BTreeSet::new()
    })
}

struct PollReads {
    read: Vec<(GmailThreadId, Option<ReadThread>)>,
    deferred: Vec<GmailThreadId>,
}

impl PollReads {
    fn add(
        &mut self,
        account: AccountId,
        thread: &GmailThreadId,
        answer: Result<ReadThread, GmailError>,
    ) -> Result<(), GmailError> {
        match answer {
            Ok(answer) => self.read.push((thread.clone(), Some(answer))),
            Err(e) if *e.outcome() == Outcome::NotFound => self.read.push((thread.clone(), None)),
            Err(e) if *e.outcome() == Outcome::InvalidGrant => return Err(e),
            Err(e) => {
                tracing::info!(account = account.0, outcome = %e.outcome(), "a poll's thread read failed; queueing it");
                self.deferred.push(thread.clone());
            }
        }
        Ok(())
    }
}

async fn read_threads(
    ctx: &AccountCtx,
    to_read: &BTreeSet<GmailThreadId>,
) -> Result<PollReads, GmailError> {
    let none = HashSet::new();
    let answers = join_all(
        to_read
            .iter()
            .map(|t| read_thread(ctx, t, Format::Metadata, Priority::P3, &none)),
    )
    .await;
    let mut reads = PollReads {
        read: Vec::with_capacity(answers.len()),
        deferred: Vec::new(),
    };
    for (thread, answer) in to_read.iter().zip(answers) {
        reads.add(ctx.account, thread, answer)?;
    }
    Ok(reads)
}

fn queue_after_poll(
    queue: &FetchQueue,
    read: BTreeSet<GmailThreadId>,
    deferred: Vec<GmailThreadId>,
) {
    for thread in read.into_iter().filter(|t| !deferred.contains(t)) {
        queue.push(POLL_BODIES, Work::Body { thread });
    }
    for thread in deferred {
        queue.push(
            POLL_META,
            Work::Meta {
                thread,
                history: None,
                body: Some(POLL_BODIES),
            },
        );
    }
}

async fn reconcile_drafts(
    ctx: &AccountCtx,
    due: &mut Option<Occasion>,
    changed: bool,
) -> Result<(), GmailError> {
    let occasion = match (*due, changed) {
        (Some(occasion), _) => occasion,
        (None, true) => Occasion::Poll,
        (None, false) => return Ok(()),
    };
    *due = Some(occasion);
    reconcile(ctx, occasion).await?;
    *due = None;
    Ok(())
}

async fn history_since(
    ctx: &AccountCtx,
    start: HistoryId,
) -> Result<Option<(Vec<sm_gmail::HistoryRecord>, HistoryId)>, GmailError> {
    let mut records = Vec::new();
    let mut page: Option<PageToken> = None;
    loop {
        match ctx
            .gmail
            .list_history(Priority::P3, start, page.as_ref())
            .await
        {
            Ok(listed) => {
                records.extend(listed.history);
                match listed.next_page_token {
                    Some(next) => page = Some(next),
                    None => return Ok(Some((records, listed.history_id))),
                }
            }
            Err(e) if *e.outcome() == Outcome::NotFound => return Ok(None),
            Err(e) => return Err(e),
        }
    }
}

pub(crate) async fn store_cursor(ctx: &AccountCtx, cursor: HistoryId) {
    let account = ctx.account;
    let stored = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, _| {
            tx.accounts().set_history(account, cursor)
        })
        .await;
    if let Err(e) = stored {
        tracing::warn!(account = account.0, error = %e, "couldn't store the history cursor");
    }
}
