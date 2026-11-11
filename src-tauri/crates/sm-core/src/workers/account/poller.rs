//! The connect sync and history polling (SPEC Sync): connect once, then poll
//! `history.list` every 45 s, on focus, and when asked.

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
use crate::Error;
use crate::notify::Arrivals;
use crate::read::read;
use crate::workers::drafts::{Occasion, reconcile, stops_the_account};
use crate::workers::snooze;
use sm_domain::AccountId;
use sm_store::ReadPool;

/// How often history is polled while Gmail answers.
const POLL_EVERY: Duration = Duration::from_secs(45);
/// Every this many polls, `sendAs` is read again.
const SEND_AS_EVERY: u32 = 20;
/// A gap in app time this long between polls means the Mac slept.
const WOKE_AFTER: Duration = Duration::from_secs(3 * 45);

/// The account's sync loop: the connect sync if it never finished, else a
/// resume; then a poll at once and on every interval, kick, and focus.
pub(crate) async fn run(ctx: Arc<AccountCtx>) {
    // A draft reconcile a sync or poll asked for that failed: the next poll
    // runs it, as that sync or poll would have, since its history no longer
    // says drafts changed.
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

/// Before the first poll: a connected account resumes, and one that is not
/// runs the connect sync until it lands. Breaks when the account is not to
/// be polled: it can't be read, or its grant was refused.
async fn resume_or_connect(ctx: &AccountCtx, drafts_due: &mut Option<Occasion>) -> ControlFlow<()> {
    match is_connected(&ctx.deps.pool, ctx.account).await {
        Ok(true) => {
            queue_inbox(&ctx.queue);
            queue_owed(ctx).await;
            // A draft reconcile the last run owed may have been cut off by
            // the quit: the first poll runs one.
            *drafts_due = Some(Occasion::Poll);
            ControlFlow::Continue(())
        }
        Ok(false) => connect_until_done(ctx, drafts_due).await,
        Err(e) => {
            tracing::error!(account = ctx.account.0, error = %e, "couldn't read the account; not syncing it");
            ControlFlow::Break(())
        }
    }
}

/// The connect sync, tried again every `OFFLINE_RETRY` until it lands.
/// Breaks when the grant is refused: the account signs out.
async fn connect_until_done(
    ctx: &AccountCtx,
    drafts_due: &mut Option<Occasion>,
) -> ControlFlow<()> {
    loop {
        match connect(ctx, drafts_due).await {
            Ok(()) => return ControlFlow::Continue(()),
            Err(e) if *e.outcome() == Outcome::InvalidGrant => {
                ctx.health.failed(&e);
                ctx.signed_out();
                return ControlFlow::Break(());
            }
            Err(e) => {
                tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "the connect sync failed; trying again");
                ctx.health.failed(&e);
                ctx.poll.wait(ctx.clock(), OFFLINE_RETRY).await;
            }
        }
    }
}

/// One poll, and what it says of the account's health. Breaks when the
/// grant is refused: the account signs out.
async fn poll_once(ctx: &AccountCtx, drafts_due: &mut Option<Occasion>) -> ControlFlow<()> {
    match poll(ctx, drafts_due).await {
        Ok(()) => ctx.health.succeeded(),
        Err(e) if *e.outcome() == Outcome::InvalidGrant => {
            ctx.health.failed(&e);
            ctx.signed_out();
            return ControlFlow::Break(());
        }
        Err(e) => {
            tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "a poll failed");
            ctx.health.failed(&e);
        }
    }
    ControlFlow::Continue(())
}

/// After a wait from `last` to `now` long enough that the Mac slept: timers
/// stood still meanwhile and the wall clock did not, so snoozes and sends
/// that came due go now.
async fn wake_if_slept(ctx: &AccountCtx, last: Millis, now: Millis) {
    let gap = u64::try_from(now.0.saturating_sub(last.0)).unwrap_or(0);
    if Duration::from_millis(gap) > WOKE_AFTER {
        tracing::info!(account = ctx.account.0, "woke from sleep");
        snooze::wake_due(ctx.writer(), now).await;
        ctx.outbox.kick();
    }
}

/// Waits for the next poll: `POLL_EVERY` while Gmail answers, or a kick.
/// Offline, polls are how the app finds out Gmail can be reached again
/// (SPEC Offline), so they come every `OFFLINE_RETRY`, and a network that
/// any call found gone meanwhile cuts the long wait short.
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

async fn is_connected(pool: &Arc<ReadPool>, account: AccountId) -> Result<bool, Error> {
    read(pool, move |r| {
        Ok(r.accounts().get(account)?.is_some_and(|a| a.connected))
    })
    .await
}

/// The connect sync: the profile's cursor, `sendAs`, labels, then the
/// lists to read (Inbox, starred, recent sent, drafts) and the contacts
/// backfill. Marked done only at the end, so a launch before that runs it
/// again.
async fn connect(ctx: &AccountCtx, drafts_due: &mut Option<Occasion>) -> Result<(), GmailError> {
    ctx.health.syncing();
    let profile = ctx.gmail.get_profile(Priority::P3).await?;
    refresh_send_as(ctx).await?;
    refresh_labels(ctx).await?;
    store_connect_cursor(ctx, profile.history_id).await;
    queue_inbox(&ctx.queue);
    queue_starred_and_sent(&ctx.queue);
    // A draft that fails to read does not hold up the connect sync: the
    // first poll runs the reconcile again. No connection does.
    match reconcile(ctx, Occasion::Sync).await {
        Ok(()) => {}
        Err(e) if stops_the_account(&e) => return Err(e),
        Err(e) => {
            tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "the connect sync's draft reconcile failed; the first poll runs it");
            *drafts_due = Some(Occasion::Sync);
        }
    }
    ctx.queue.push(BACKFILL, Work::Backfill { page: None });
    mark_connected(ctx).await;
    Ok(())
}

/// Stores the connect sync's cursor, unless the account has one. A retry
/// after a failed attempt keeps that attempt's cursor: the lists it read
/// predate a newer one, so polling from the newer one would miss what
/// changed in between.
async fn store_connect_cursor(ctx: &AccountCtx, cursor: HistoryId) {
    let (account, now) = (ctx.account, ctx.clock().now());
    let stored = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, _| {
            let Some(record) = tx.read().accounts().get(account)? else {
                return Ok(());
            };
            if record.history_id.is_none() {
                tx.accounts().set_history(account, cursor, now)?;
            }
            Ok::<_, sm_store::StoreError>(())
        })
        .await;
    if let Err(e) = stored {
        tracing::warn!(account = account.0, error = %e, "couldn't store the connect cursor");
    }
}

/// Marks the connect sync done: a launch from now on resumes.
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

/// At launch: the threads a full resync owed a metadata read when the app
/// quit (their history id cleared), read in the background.
async fn queue_owed(ctx: &AccountCtx) {
    let account = ctx.account;
    let owed = ctx.read(move |r| r.threads().owed_reads(account)).await;
    match owed {
        Ok(owed) => {
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
        Err(e) => {
            tracing::warn!(account = account.0, error = %e, "couldn't read the threads owed a read");
        }
    }
}

/// Inbox, newest first: metadata, then bodies. Unchanged threads are
/// skipped by their history id, so after a restart this only fetches what
/// changed and the bodies not reached yet.
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

/// Starred threads and the last 90 days of Sent, behind the Inbox.
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

/// One poll from the stored cursor. A cursor Gmail no longer keeps runs the
/// full resync.
async fn poll(ctx: &AccountCtx, drafts_due: &mut Option<Occasion>) -> Result<(), GmailError> {
    let account = ctx.account;
    let start = ctx
        .read(move |r| Ok(r.accounts().get(account)?.and_then(|a| a.history_id)))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(account = account.0, error = %e, "couldn't read the history cursor");
            None
        });
    let Some(start) = start else {
        return Ok(());
    };
    // Before history is read: ops that land from now on are kept on top of it.
    let ticket = ctx.lane.landed.read();
    let Some((records, latest)) = history_since(ctx, start).await? else {
        return full_resync(ctx, drafts_due).await;
    };
    if records.is_empty() {
        if latest != start {
            store_cursor(ctx, latest).await;
        }
        return reconcile_drafts(ctx, drafts_due, false).await;
    }
    let batch = HistoryBatch::new(records);
    let drafts_changed = batch.drafts_changed;
    let known = known_messages(ctx, batch.labels.keys().cloned().collect()).await;
    let to_read = batch.threads_to_read(|id| known.contains(id));
    let (read, deferred) = read_threads(ctx, &to_read).await?;
    let now = ctx.clock().now();
    let notifier = Arc::clone(&ctx.deps.notifier);
    let applied = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, state| {
            let polled = Polled {
                account,
                batch: &batch,
                read,
                deferred: &deferred,
                cursor: latest,
                now,
                ticket: &ticket,
            };
            let added = apply_history(tx, state, polled)?;
            let focused = notifier.is_focused();
            let arrivals = Arrivals {
                stored: &added,
                reported: &batch.added,
            };
            crate::notify::new_mail(tx.read(), state, account, arrivals, focused)?;
            // Back to the poll, which queues them once this has landed.
            Ok::<_, sm_store::StoreError>(deferred)
        })
        .await;
    match applied {
        Ok(deferred) => queue_after_poll(&ctx.queue, to_read, deferred),
        Err(e) => {
            tracing::warn!(account = account.0, error = %e, "couldn't apply a poll's history");
        }
    }
    reconcile_drafts(ctx, drafts_due, drafts_changed).await
}

/// Which of the messages a poll's history names are stored. None, when they
/// can't be read: the poll then reads every thread its history names.
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

/// Reads the metadata of a poll's threads, all at once. Returns each thread
/// read with its answer, `None` for one Gmail no longer has, and the threads
/// whose read failed. Only a refused grant fails the poll.
async fn read_threads(
    ctx: &AccountCtx,
    to_read: &BTreeSet<GmailThreadId>,
) -> Result<(Vec<(GmailThreadId, Option<ReadThread>)>, Vec<GmailThreadId>), GmailError> {
    let none = HashSet::new();
    let answers = join_all(
        to_read
            .iter()
            .map(|t| read_thread(ctx, t, Format::Metadata, Priority::P3, &none)),
    )
    .await;
    let mut read = Vec::with_capacity(answers.len());
    let mut deferred = Vec::new();
    for (thread, answer) in to_read.iter().zip(answers) {
        match answer {
            Ok(answer) => read.push((thread.clone(), Some(answer))),
            Err(e) if *e.outcome() == Outcome::NotFound => read.push((thread.clone(), None)),
            Err(e) if *e.outcome() == Outcome::InvalidGrant => return Err(e),
            // One failed read does not hold up the rest of the poll: the
            // fetch queue reads the thread, retrying as it does.
            Err(e) => {
                tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "a poll's thread read failed; queueing it");
                deferred.push(thread.clone());
            }
        }
    }
    Ok((read, deferred))
}

/// After a poll lands: the bodies of the threads it read, and the threads
/// whose read failed, which the fetch queue reads again.
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

/// Reconciles the drafts when the poll's history says a draft was added or
/// an old version deleted, or a poll before asked and failed.
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

/// Every history record after `start`, and the newest cursor; `None` when
/// Gmail answers 404 because the cursor is too old.
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

/// Stores a new cursor with nothing else.
pub(crate) async fn store_cursor(ctx: &AccountCtx, cursor: HistoryId) {
    let (account, now) = (ctx.account, ctx.clock().now());
    let stored = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, _| {
            tx.accounts().set_history(account, cursor, now)
        })
        .await;
    if let Err(e) = stored {
        tracing::warn!(account = account.0, error = %e, "couldn't store the history cursor");
    }
}
