//! The connect sync and history polling (SPEC Sync): connect once, then poll
//! `history.list` every 45 s, on focus, and when asked.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use ub_domain::{GmailMessageId, GmailThreadId, SystemLabel};
use ub_gmail::{Format, GmailError, HistoryId, Outcome, PageToken, Priority};
use ub_store::Priority as WritePriority;

use super::ctx::{AccountCtx, OFFLINE_RETRY};
use super::history::HistoryBatch;
use super::ingest::{Polled, apply_history};
use super::profile::{refresh_labels, refresh_send_as};
use super::queue::{
    BACKFILL, INBOX_BODIES, INBOX_META, LATE, Listing, OTHER_META, POLL_BODIES, POLL_META, Work,
};
use super::read::read_thread;
use super::resync::full_resync;
use crate::Error;
use crate::read::read;
use crate::workers::drafts::{Occasion, reconcile};
use crate::workers::snooze;
use ub_domain::AccountId;
use ub_store::ReadPool;

/// How often history is polled while Gmail answers.
const POLL_EVERY: Duration = Duration::from_secs(45);
/// Every this many polls, `sendAs` is read again.
const SEND_AS_EVERY: u32 = 20;
/// A gap in app time this long between polls means the Mac slept.
const WOKE_AFTER: Duration = Duration::from_secs(3 * 45);

/// The account's sync loop: the connect sync if it never finished, else a
/// resume; then a poll at once and on every interval, kick, and focus.
pub(crate) async fn run(ctx: Arc<AccountCtx>) {
    match is_connected(&ctx.deps.pool, ctx.account).await {
        Ok(true) => queue_inbox(&ctx),
        Ok(false) => loop {
            match connect(&ctx).await {
                Ok(()) => break,
                Err(e) if *e.outcome() == Outcome::InvalidGrant => {
                    ctx.health.failed(&e);
                    ctx.signed_out();
                    return;
                }
                Err(e) => {
                    tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "the connect sync failed; trying again");
                    ctx.health.failed(&e);
                    ctx.poll.wait(ctx.clock(), OFFLINE_RETRY).await;
                }
            }
        },
        Err(e) => {
            tracing::error!(account = ctx.account.0, error = %e, "couldn't read the account; not syncing it");
            return;
        }
    }
    let mut polls = 0u32;
    let mut last = ctx.clock().now();
    // A draft reconcile a poll asked for that failed: the next poll runs it,
    // since its history no longer says drafts changed.
    let mut drafts_due = false;
    loop {
        match poll(&ctx, &mut drafts_due).await {
            Ok(()) => ctx.health.succeeded(),
            Err(e) if *e.outcome() == Outcome::InvalidGrant => {
                ctx.health.failed(&e);
                ctx.signed_out();
                return;
            }
            Err(e) => {
                tracing::info!(account = ctx.account.0, outcome = %e.outcome(), "a poll failed");
                ctx.health.failed(&e);
            }
        }
        polls = polls.wrapping_add(1);
        if polls.is_multiple_of(SEND_AS_EVERY)
            && let Err(e) = refresh_send_as(&ctx).await
        {
            ctx.health.failed(&e);
        }
        wait_to_poll(&ctx).await;
        let now = ctx.clock().now();
        let gap = u64::try_from(now.0.saturating_sub(last.0)).unwrap_or(0);
        if Duration::from_millis(gap) > WOKE_AFTER {
            // Timers stood still while the Mac slept; the wall clock did not.
            // Snoozes and sends that came due meanwhile go now.
            tracing::info!(account = ctx.account.0, "woke from sleep");
            snooze::wake_due(ctx.writer(), now).await;
            ctx.outbox.kick();
        }
        last = now;
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
async fn connect(ctx: &AccountCtx) -> Result<(), GmailError> {
    ctx.health.syncing();
    let profile = ctx.gmail.get_profile(Priority::P3).await?;
    refresh_send_as(ctx).await?;
    refresh_labels(ctx).await?;
    let (account, now) = (ctx.account, ctx.clock().now());
    // A retry after a failed attempt keeps that attempt's cursor: the lists
    // it read predate a newer one, so polling from the newer one would miss
    // what changed in between.
    let cursor = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, _| {
            let Some(record) = tx.read().accounts().get(account)? else {
                return Ok(());
            };
            if record.history_id.is_none() {
                tx.accounts()
                    .set_history(account, profile.history_id, now)?;
            }
            Ok::<_, ub_store::StoreError>(())
        })
        .await;
    if let Err(e) = cursor {
        tracing::warn!(account = account.0, error = %e, "couldn't store the connect cursor");
    }
    queue_inbox(ctx);
    queue_starred_and_sent(ctx);
    reconcile(ctx, Occasion::Sync).await?;
    ctx.queue.push(BACKFILL, Work::Backfill { page: None });
    let connected = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, _| {
            tx.accounts().set_connected(account)
        })
        .await;
    if let Err(e) = connected {
        tracing::warn!(account = account.0, error = %e, "couldn't mark the account connected");
    }
    Ok(())
}

/// Inbox, newest first: metadata, then bodies. Unchanged threads are
/// skipped by their history id, so after a restart this only fetches what
/// changed and the bodies not reached yet.
pub(crate) fn queue_inbox(ctx: &AccountCtx) {
    ctx.queue.push(
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
pub(crate) fn queue_starred_and_sent(ctx: &AccountCtx) {
    ctx.queue.push(
        OTHER_META,
        Work::List {
            listing: Listing::Label(SystemLabel::Starred.label_id()),
            page: None,
            meta: OTHER_META,
            body: Some(LATE),
        },
    );
    ctx.queue.push(
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
async fn poll(ctx: &AccountCtx, drafts_due: &mut bool) -> Result<(), GmailError> {
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
        return full_resync(ctx).await;
    };
    if records.is_empty() {
        if latest != start {
            store_cursor(ctx, latest).await;
        }
        return reconcile_drafts(ctx, drafts_due, false).await;
    }
    let batch = HistoryBatch::new(records);
    let drafts_changed = batch.drafts_changed;
    let named: Vec<GmailMessageId> = batch.labels.keys().cloned().collect();
    let known: BTreeSet<GmailMessageId> = ctx
        .read(move |r| {
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
        });
    let to_read = batch.threads_to_read(|id| known.contains(id));
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
                tracing::info!(account = account.0, outcome = %e.outcome(), "a poll's thread read failed; queueing it");
                deferred.push(thread.clone());
            }
        }
    }
    let now = ctx.clock().now();
    let notifier = Arc::clone(&ctx.deps.notifier);
    let kept = deferred.clone();
    let applied = ctx
        .writer()
        .call(WritePriority::Sync, move |tx, state| {
            let polled = Polled {
                account,
                batch: &batch,
                read,
                deferred: &kept,
                cursor: latest,
                now,
                ticket: &ticket,
            };
            let added = apply_history(tx, state, polled)?;
            let focused = notifier.is_focused();
            crate::notify::new_mail(tx.read(), state, account, &added, &batch.added, focused)
        })
        .await;
    match applied {
        Ok(()) => queue_after_poll(ctx, to_read, &deferred),
        Err(e) => {
            tracing::warn!(account = account.0, error = %e, "couldn't apply a poll's history");
        }
    }
    reconcile_drafts(ctx, drafts_due, drafts_changed).await
}

/// After a poll lands: the bodies of the threads it read, and the threads
/// whose read failed, which the fetch queue reads again.
fn queue_after_poll(ctx: &AccountCtx, read: BTreeSet<GmailThreadId>, deferred: &[GmailThreadId]) {
    for thread in read.into_iter().filter(|t| !deferred.contains(t)) {
        ctx.queue.push(POLL_BODIES, Work::Body { thread });
    }
    for thread in deferred.iter().cloned() {
        ctx.queue.push(
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
    due: &mut bool,
    changed: bool,
) -> Result<(), GmailError> {
    if !(changed || *due) {
        return Ok(());
    }
    *due = true;
    reconcile(ctx, Occasion::Poll).await?;
    *due = false;
    Ok(())
}

/// Every history record after `start`, and the newest cursor; `None` when
/// Gmail answers 404 because the cursor is too old.
async fn history_since(
    ctx: &AccountCtx,
    start: HistoryId,
) -> Result<Option<(Vec<ub_gmail::HistoryRecord>, HistoryId)>, GmailError> {
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
