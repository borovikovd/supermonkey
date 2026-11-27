use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::domain::{
    AccountId, GmailMessageId, GmailThreadId, HistoryId, LabelId, LabelSet, Millis, SendAsAddress,
    SyncStatus, SystemLabel, ThreadId,
};
use crate::gmail::{
    Backoff, Format, Gmail, GmailError, LabelType, Outcome, Priority as GmailPriority, ThreadQuery,
};
use crate::mime::parse_message;
use crate::store::{ContactSighting, Priority as StorePriority, ReadPool, StoreError, Writer};

use super::super::drafts::DraftFiles;
use super::super::events::{Effect, NewMailAlert};
use super::super::network::Network;
use super::super::runtime::{Activity, Clock, Kick, Tasks};
use super::super::writer_state::{SubOp, WriterState};
use super::drafts::reconcile_drafts_with_gmail;
use super::ingest::{IngestResult, fetch_and_ingest_thread};
use super::ops::wait_kick;
use super::queue::{FetchJob, FetchPriority, FetchQueue};

pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(45);
pub(crate) const RETRYING_THRESHOLD_MS: i64 = 30_000;
pub(crate) const SLEEP_JUMP_THRESHOLD_MS: i64 = 135_000;
const FETCH_CONCURRENCY: usize = 4;
const CONTACTS_BACKFILL_MAX: usize = 250;

#[derive(Debug, Default)]
struct HealthState {
    first_failure: Option<Millis>,
    current_status: Option<SyncStatus>,
}

pub(crate) struct AccountSlot {
    pub(crate) account: AccountId,
    pub(crate) email: String,
    gmail: RwLock<Option<(u64, Arc<Gmail>)>>,
    health: Mutex<HealthState>,
    pub(crate) network: Network,
    tasks: Tasks,
    pub(crate) fetch_queue: FetchQueue,
    pub(crate) poll_kick: Kick,
    pub(crate) mail_ops_kick: Kick,
    pub(crate) draft_ops_kick: Kick,
    pub(crate) outbox_kick: Kick,
    pub(crate) writer: Writer<WriterState>,
    pub(crate) pool: Arc<ReadPool>,
    pub(crate) clock: Clock,
    pub(crate) activity: Activity,
    pub(crate) files: DraftFiles,
}

impl std::fmt::Debug for AccountSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountSlot")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl AccountSlot {
    #[expect(
        clippy::too_many_arguments,
        reason = "account slot holds shared runtime dependencies for all per-account workers"
    )]
    #[must_use]
    pub(crate) fn new(
        account: AccountId,
        email: String,
        gmail: Option<Arc<Gmail>>,
        network: Network,
        tasks: Tasks,
        writer: Writer<WriterState>,
        pool: Arc<ReadPool>,
        clock: Clock,
        activity: Activity,
        files: DraftFiles,
    ) -> Self {
        let initial = gmail.map(|g| (1_u64, g));
        Self {
            account,
            email,
            gmail: RwLock::new(initial),
            health: Mutex::new(HealthState::default()),
            network,
            tasks,
            fetch_queue: FetchQueue::new(),
            poll_kick: Kick::new(),
            mail_ops_kick: Kick::new(),
            draft_ops_kick: Kick::new(),
            outbox_kick: Kick::new(),
            writer,
            pool,
            clock,
            activity,
            files,
        }
    }

    #[must_use]
    pub(crate) fn gmail(&self) -> Option<(u64, Arc<Gmail>)> {
        self.gmail
            .read()
            .as_ref()
            .map(|(grant_gen, g)| (*grant_gen, Arc::clone(g)))
    }

    pub(crate) fn set_gmail(&self, next: Arc<Gmail>) -> u64 {
        let mut slot = self.gmail.write();
        let next_gen = slot.as_ref().map_or(1, |(g, _)| g.saturating_add(1));
        *slot = Some((next_gen, next));
        drop(slot);
        self.health.lock().first_failure = None;
        next_gen
    }

    fn clear_gmail_if_gen(&self, grant_gen: u64) -> bool {
        let mut slot = self.gmail.write();
        if slot.as_ref().is_some_and(|(cur, _)| *cur == grant_gen) {
            *slot = None;
            true
        } else {
            false
        }
    }

    #[must_use]
    pub(crate) fn is_online(&self) -> bool {
        self.network.is_online()
    }

    #[cfg(any(test, feature = "fake"))]
    #[must_use]
    pub(crate) fn has_failure(&self) -> bool {
        self.health.lock().first_failure.is_some()
    }

    pub(crate) fn kick_poll(&self) {
        self.poll_kick.wake(&self.activity);
    }

    pub(crate) fn kick_ops(&self) {
        self.mail_ops_kick.wake(&self.activity);
        self.draft_ops_kick.wake(&self.activity);
    }

    pub(crate) fn kick_outbox(&self) {
        self.outbox_kick.wake(&self.activity);
    }

    pub(crate) async fn report_success(&self, grant_gen: u64) {
        let cur = self.gmail.read().as_ref().map(|(g, _)| *g);
        if cur != Some(grant_gen) {
            return;
        }
        let was_retrying = {
            let mut h = self.health.lock();
            h.first_failure = None;
            h.current_status == Some(SyncStatus::Retrying)
        };
        if was_retrying {
            self.set_status(SyncStatus::Idle).await;
        }
    }

    pub(crate) async fn set_status(&self, status: SyncStatus) {
        {
            let mut h = self.health.lock();
            if h.current_status == Some(status) {
                return;
            }
            h.current_status = Some(status);
        }
        let acct = self.account;
        super::super::log_err(
            "update account sync status",
            self.writer
                .call(StorePriority::Sync, move |tx, _state| {
                    tx.set_sync_status(acct, status)
                })
                .await,
        );
    }

    pub(crate) async fn mark_auth_failed(&self, grant_gen: u64) {
        if !self.clear_gmail_if_gen(grant_gen) {
            return;
        }
        self.fetch_queue.clear();
        {
            let mut h = self.health.lock();
            h.first_failure = None;
            h.current_status = Some(SyncStatus::NeedsSignin);
        }
        let acct = self.account;
        super::super::log_err(
            "mark account needs_signin",
            self.writer
                .call(StorePriority::Sync, move |tx, state| {
                    tx.set_needs_signin(acct, true)?;
                    tx.set_sync_status(acct, SyncStatus::NeedsSignin)?;
                    state.stage_sub(SubOp::StopSearching(acct));
                    Ok::<_, StoreError>(())
                })
                .await,
        );
    }

    pub(crate) async fn handle_gmail_error(
        self: &Arc<Self>,
        grant_gen: u64,
        err: &GmailError,
        backoff: &mut Backoff,
    ) -> Option<Duration> {
        if matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized) {
            self.mark_auth_failed(grant_gen).await;
            return None;
        }
        self.network.lost_if_unreachable(err).await;
        if !self.is_online() {
            self.health.lock().first_failure = None;
            self.set_status(SyncStatus::Offline).await;
            return None;
        }
        let now = self.clock.now();
        let (show_retrying, spawn_timer) = {
            let mut h = self.health.lock();
            if let Some(first) = h.first_failure {
                (
                    now.0.saturating_sub(first.0) >= RETRYING_THRESHOLD_MS,
                    false,
                )
            } else {
                h.first_failure = Some(now);
                (false, true)
            }
        };
        if spawn_timer {
            let slot = Arc::clone(self);
            let cancel = self.tasks.token();
            self.tasks.spawn(async move {
                let dur =
                    Duration::from_millis(u64::try_from(RETRYING_THRESHOLD_MS).unwrap_or(30_000));
                let sleep = slot.clock.sleep(dur);
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = sleep => {}
                }
                let _busy = slot.activity.enter();
                let still_failing = slot.health.lock().first_failure == Some(now);
                if still_failing && slot.is_online() && slot.gmail().is_some() {
                    slot.set_status(SyncStatus::Retrying).await;
                }
            });
        }
        if show_retrying {
            self.set_status(SyncStatus::Retrying).await;
        }
        Some(backoff.next_delay(err.retry_after()))
    }
}

pub(crate) fn spawn_fetch_workers(slot: &Arc<AccountSlot>, tasks: &Tasks) {
    for _ in 0..FETCH_CONCURRENCY {
        let s = Arc::clone(slot);
        let cancel = tasks.token();
        tasks.spawn(async move {
            run_fetch_worker(s, cancel).await;
        });
    }
}

async fn handle_fetch_success(
    slot: &AccountSlot,
    gmail: &Gmail,
    grant_gen: u64,
    ingest: IngestResult,
    cancel: &CancellationToken,
) {
    slot.report_success(grant_gen).await;
    if ingest.unknown_labels
        && let Some(res) = cancel
            .run_until_cancelled(refresh_labels_guarded(slot, gmail, GmailPriority::P2))
            .await
    {
        super::super::log_err("refresh unknown labels", res);
    }
    if !ingest.alerts.is_empty() {
        let acct = slot.account;
        let email = slot.email.clone();
        let alerts = ingest.alerts;
        super::super::log_err(
            "emit new mail alerts",
            slot.writer
                .call(StorePriority::Sync, move |_tx, state| {
                    state.emit(Effect::NotifyNewMail {
                        account: acct,
                        email,
                        items: alerts,
                    });
                    Ok::<_, StoreError>(())
                })
                .await,
        );
    }
}

async fn run_fetch_worker(slot: Arc<AccountSlot>, cancel: CancellationToken) {
    while let Some(job) = slot.fetch_queue.pop(&cancel).await {
        let Some(settled) = fetch_one(&slot, &job, &cancel).await else {
            slot.fetch_queue.finish(&job, false);
            return;
        };
        slot.fetch_queue.finish(&job, settled);
    }
}

async fn fetch_one(
    slot: &Arc<AccountSlot>,
    job: &FetchJob,
    cancel: &CancellationToken,
) -> Option<bool> {
    if !slot.is_online() {
        return Some(false);
    }
    let Some((grant_gen, gmail)) = slot.gmail() else {
        return Some(false);
    };
    let att_dir = slot.files.attachments_dir();
    let res = cancel
        .run_until_cancelled(fetch_and_ingest_thread(
            slot.account,
            &gmail,
            &slot.pool,
            &slot.writer,
            &slot.clock,
            Some(att_dir.as_path()),
            &job.gmail_id,
            job.format,
            job.priority.gmail(),
            job.notify_new_mail,
        ))
        .await?;
    match res {
        Ok(ingest) => {
            handle_fetch_success(slot, &gmail, grant_gen, ingest, cancel).await;
            Some(true)
        }
        Err(err) => {
            slot.network.lost_if_unreachable(&err).await;
            if matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized) {
                slot.mark_auth_failed(grant_gen).await;
                return Some(false);
            }
            if !slot.is_online() {
                slot.set_status(SyncStatus::Offline).await;
            }
            Some(!err.is_transient())
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "sync loop manages polling timer, sleep-jump detection, connect/incremental dispatch, and backoff"
)]
pub(crate) async fn run_sync_loop(slot: Arc<AccountSlot>, tasks: Tasks, cancel: CancellationToken) {
    let mut backoff = Backoff::new();
    let mut wait: Option<Duration> = None;
    let mut last_wall = slot.clock.now();
    let mut backfill_queued = false;
    let mut pending_alerts: Vec<NewMailAlert> = Vec::new();

    loop {
        if cancel.is_cancelled() {
            return;
        }
        let Some(guard) = wait_kick(&slot.poll_kick, &slot, &mut wait, &cancel).await else {
            return;
        };

        let now = slot.clock.now();
        let jumped = now.0.saturating_sub(last_wall.0) >= SLEEP_JUMP_THRESHOLD_MS;
        last_wall = now;
        if jumped {
            super::super::log_err(
                "kick snooze after wall clock jump",
                slot.writer
                    .call(StorePriority::Sync, |_tx, state| {
                        state.emit(Effect::KickSnooze);
                        Ok::<_, StoreError>(())
                    })
                    .await,
            );
        }

        if !slot.is_online() {
            slot.set_status(SyncStatus::Offline).await;
            drop(guard);
            continue;
        }
        let Some((grant_gen, gmail)) = slot.gmail() else {
            pending_alerts.clear();
            drop(guard);
            continue;
        };

        let Some(acct_row) = slot.pool.with(|db| db.account(slot.account)).ok().flatten() else {
            drop(guard);
            return;
        };

        let maybe_result = if acct_row.connected {
            cancel
                .run_until_cancelled(run_incremental_poll(
                    &slot,
                    &gmail,
                    acct_row.history_id,
                    &mut pending_alerts,
                    &cancel,
                ))
                .await
        } else {
            pending_alerts.clear();
            let res = cancel
                .run_until_cancelled(run_connect_sync(
                    &slot,
                    &gmail,
                    acct_row.history_id,
                    &cancel,
                ))
                .await;
            if matches!(&res, Some(Ok(()))) && !backfill_queued {
                backfill_queued = true;
                spawn_contacts_backfill(Arc::clone(&slot), Arc::clone(&gmail), &tasks);
            }
            res
        };

        let Some(result) = maybe_result else {
            drop(guard);
            return;
        };

        wait =
            settle_sync_result(&slot, grant_gen, result, &mut backoff, &mut pending_alerts).await;
        drop(guard);
    }
}

async fn settle_sync_result(
    slot: &Arc<AccountSlot>,
    grant_gen: u64,
    result: Result<(), GmailError>,
    backoff: &mut Backoff,
    pending_alerts: &mut Vec<NewMailAlert>,
) -> Option<Duration> {
    match result {
        Ok(()) => {
            backoff.reset();
            slot.report_success(grant_gen).await;
            slot.set_status(SyncStatus::Idle).await;
            Some(POLL_INTERVAL)
        }
        Err(_) if slot.gmail().is_none() => {
            pending_alerts.clear();
            None
        }
        Err(err) => {
            let next = slot.handle_gmail_error(grant_gen, &err, backoff).await;
            if next.is_none() && slot.is_online() && slot.gmail().is_some() {
                Some(POLL_INTERVAL)
            } else {
                next
            }
        }
    }
}

async fn ensure_connect_history(
    slot: &AccountSlot,
    gmail: &Gmail,
    existing_history: Option<HistoryId>,
) -> Result<HistoryId, GmailError> {
    if let Some(h) = existing_history {
        return Ok(h);
    }
    let profile = gmail.get_profile(GmailPriority::P2).await?;
    let acct = slot.account;
    let h = profile.history_id;
    super::super::log_err(
        "persist initial history cursor",
        slot.writer
            .call(StorePriority::Sync, move |tx, _state| {
                tx.set_history(acct, h)
            })
            .await,
    );
    Ok(h)
}

fn enqueue_full_batch(slot: &AccountSlot, ids: &[GmailThreadId], priority: FetchPriority) {
    for id in ids {
        slot.fetch_queue.enqueue(
            id.clone(),
            Format::Full,
            priority,
            false,
            None,
            &slot.activity,
        );
    }
}

fn known_thread_ids(slot: &AccountSlot) -> HashSet<GmailThreadId> {
    slot.pool
        .with(|db| db.thread_sync_states(slot.account))
        .unwrap_or_default()
        .into_iter()
        .map(|state| state.gmail_id)
        .collect()
}

#[expect(
    clippy::too_many_lines,
    reason = "connect sync executes the phased inbox, drafts, starred, and sent initial sync pipeline"
)]
async fn run_connect_sync(
    slot: &AccountSlot,
    gmail: &Gmail,
    existing_history: Option<HistoryId>,
    cancel: &CancellationToken,
) -> Result<(), GmailError> {
    slot.set_status(SyncStatus::Syncing).await;
    let start_history = ensure_connect_history(slot, gmail, existing_history).await?;

    refresh_labels_and_send_as(slot, gmail, GmailPriority::P2).await?;

    let inbox_ids = list_inbox_thread_ids(gmail, GmailPriority::P2).await?;

    let known = known_thread_ids(slot);
    let inbox_unread: Vec<GmailThreadId> = inbox_ids
        .iter()
        .filter(|id| !known.contains(*id))
        .cloned()
        .collect();
    let inbox_read = fetch_batch_and_wait(
        slot,
        &inbox_unread,
        Format::Metadata,
        FetchPriority::ConnectInbox,
        cancel,
    )
    .await;

    if cancel.is_cancelled() {
        return Ok(());
    }
    if !inbox_read {
        return Err(threads_left_unread());
    }
    enqueue_full_batch(slot, &inbox_unread, FetchPriority::ConnectSecondary);

    let draft_threads = reconcile_drafts_with_gmail(
        slot.account,
        gmail,
        &slot.pool,
        &slot.writer,
        GmailPriority::P4,
    )
    .await?;

    let (starred_ids, sent_ids) =
        list_starred_and_recent_sent_thread_ids(gmail, GmailPriority::P5).await?;

    let inbox_set: HashSet<&GmailThreadId> = inbox_ids.iter().collect();
    let mut secondary: Vec<GmailThreadId> = Vec::new();
    let mut seen: HashSet<GmailThreadId> = HashSet::new();
    for id in draft_threads.into_iter().chain(starred_ids).chain(sent_ids) {
        if !inbox_set.contains(&id) && !known.contains(&id) && seen.insert(id.clone()) {
            secondary.push(id);
        }
    }

    let secondary_read = fetch_batch_and_wait(
        slot,
        &secondary,
        Format::Metadata,
        FetchPriority::ConnectSecondary,
        cancel,
    )
    .await;

    if cancel.is_cancelled() {
        return Ok(());
    }
    if !secondary_read {
        return Err(threads_left_unread());
    }
    enqueue_full_batch(slot, &secondary, FetchPriority::ConnectSecondary);

    let acct = slot.account;
    super::super::log_err(
        "mark account connected",
        slot.writer
            .call(StorePriority::Sync, move |tx, _state| {
                tx.set_history(acct, start_history)?;
                tx.set_connected(acct, true)?;
                Ok::<_, StoreError>(())
            })
            .await,
    );

    let mut connect_alerts = Vec::new();
    run_incremental_poll(
        slot,
        gmail,
        Some(start_history),
        &mut connect_alerts,
        cancel,
    )
    .await
}

struct HistoryChanges {
    newest_history: HistoryId,
    changed_threads: Vec<GmailThreadId>,
    seen_threads: HashSet<GmailThreadId>,
    deleted_msgs: Vec<GmailMessageId>,
}

#[expect(
    clippy::too_many_lines,
    reason = "paginates history records, deduplicates changed threads, and merges owed threads"
)]
async fn collect_history_changes(
    slot: &AccountSlot,
    gmail: &Gmail,
    start_id: HistoryId,
) -> Result<Option<HistoryChanges>, GmailError> {
    let mut page_token = None;
    let mut records = Vec::new();
    let mut newest_history = start_id;
    loop {
        let page = match gmail
            .list_history(start_id, page_token.as_ref(), GmailPriority::P2)
            .await
        {
            Ok(p) => p,
            Err(err) if err.outcome == Outcome::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        if let Some(h) = page.history_id
            && h > newest_history
        {
            newest_history = h;
        }
        records.extend(page.history);
        if page.next_page_token.is_none() {
            break;
        }
        page_token = page.next_page_token;
    }

    let mut changed_threads = Vec::new();
    let mut seen_threads = HashSet::new();
    let mut deleted_msgs = Vec::new();

    for rec in &records {
        for item in rec
            .messages_added
            .iter()
            .chain(&rec.labels_added)
            .chain(&rec.labels_removed)
        {
            if let Some(gtid) = &item.message.thread_id
                && seen_threads.insert(gtid.clone())
            {
                changed_threads.push(gtid.clone());
            }
        }
        for item in &rec.messages_deleted {
            deleted_msgs.push(item.message.id.clone());
            if let Some(gtid) = &item.message.thread_id
                && seen_threads.insert(gtid.clone())
            {
                changed_threads.push(gtid.clone());
            }
        }
    }

    let owed = slot
        .pool
        .with(|db| db.owed_threads(slot.account))
        .unwrap_or_default();
    for gtid in owed {
        if seen_threads.insert(gtid.clone()) {
            changed_threads.push(gtid);
        }
    }

    Ok(Some(HistoryChanges {
        newest_history,
        changed_threads,
        seen_threads,
        deleted_msgs,
    }))
}

fn enqueue_full_if_missing_body(
    slot: &AccountSlot,
    gmail_tid: &GmailThreadId,
    ingest: &IngestResult,
) {
    if !ingest.added_messages {
        return;
    }
    if let Some(tid) = ingest.thread_id
        && slot
            .pool
            .with(|db| Ok::<_, StoreError>(!db.threads_needing_bodies(&[tid])?.is_empty()))
            .unwrap_or(false)
    {
        slot.fetch_queue.enqueue(
            gmail_tid.clone(),
            Format::Full,
            FetchPriority::SyncNew,
            false,
            None,
            &slot.activity,
        );
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "applies history deletions, fetches changed threads, reconciles drafts, and advances cursor"
)]
async fn run_incremental_poll(
    slot: &AccountSlot,
    gmail: &Gmail,
    cursor: Option<HistoryId>,
    pending_alerts: &mut Vec<NewMailAlert>,
    cancel: &CancellationToken,
) -> Result<(), GmailError> {
    let Some(start_id) = cursor else {
        pending_alerts.clear();
        return run_full_resync(slot, gmail, cancel).await;
    };
    let Some(mut changes) = collect_history_changes(slot, gmail, start_id).await? else {
        pending_alerts.clear();
        return run_full_resync(slot, gmail, cancel).await;
    };

    if !changes.deleted_msgs.is_empty() {
        let acct = slot.account;
        let deleted = changes.deleted_msgs;
        super::super::log_err(
            "prune deleted messages from history",
            slot.writer
                .call(StorePriority::Sync, move |tx, state| {
                    for gmid in &deleted {
                        if let Some(msg) = tx.read().message_by_gmail(acct, gmid)? {
                            tx.delete_message(msg.id)?;
                        }
                    }
                    state.emit(Effect::RefreshBadge);
                    Ok::<_, StoreError>(())
                })
                .await,
        );
    }

    let att_dir = slot.files.attachments_dir();
    let mut saw_unknown_label = false;
    for gtid in &changes.changed_threads {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let res = fetch_and_ingest_thread(
            slot.account,
            gmail,
            &slot.pool,
            &slot.writer,
            &slot.clock,
            Some(att_dir.as_path()),
            gtid,
            Format::Metadata,
            GmailPriority::P2,
            true,
        )
        .await?;
        saw_unknown_label |= res.unknown_labels;
        enqueue_full_if_missing_body(slot, gtid, &res);
        pending_alerts.extend(res.alerts);
    }

    if saw_unknown_label {
        super::super::log_err(
            "refresh unknown labels during poll",
            refresh_labels_guarded(slot, gmail, GmailPriority::P2).await,
        );
    }

    let extra_draft_threads = reconcile_drafts_with_gmail(
        slot.account,
        gmail,
        &slot.pool,
        &slot.writer,
        GmailPriority::P2,
    )
    .await?;
    for gtid in extra_draft_threads {
        if changes.seen_threads.insert(gtid.clone()) {
            match fetch_and_ingest_thread(
                slot.account,
                gmail,
                &slot.pool,
                &slot.writer,
                &slot.clock,
                Some(att_dir.as_path()),
                &gtid,
                Format::Metadata,
                GmailPriority::P2,
                false,
            )
            .await
            {
                Ok(res) => enqueue_full_if_missing_body(slot, &gtid, &res),
                Err(err) => {
                    super::super::log_err("fetch draft thread during poll", Err::<(), _>(err));
                }
            }
        }
    }

    let acct = slot.account;
    let email = slot.email.clone();
    let newest = changes.newest_history;
    let alerts = std::mem::take(pending_alerts);
    super::super::log_err(
        "advance history cursor",
        slot.writer
            .call(StorePriority::Sync, move |tx, state| {
                tx.set_history(acct, newest)?;
                let mut live_alerts = Vec::with_capacity(alerts.len());
                for item in alerts {
                    if tx
                        .read()
                        .thread_row(item.thread_id)?
                        .is_some_and(|row| row.account == acct && row.in_inbox && row.unread)
                    {
                        live_alerts.push(item);
                    }
                }
                if !live_alerts.is_empty() {
                    state.emit(Effect::NotifyNewMail {
                        account: acct,
                        email,
                        items: live_alerts,
                    });
                }
                Ok::<_, StoreError>(())
            })
            .await,
    );

    Ok(())
}

async fn list_inbox_thread_ids(
    gmail: &Gmail,
    priority: GmailPriority,
) -> Result<Vec<GmailThreadId>, GmailError> {
    let inbox_label = SystemLabel::Inbox.label_id();
    list_all_thread_ids(
        gmail,
        ThreadQuery {
            q: None,
            label_id: Some(&inbox_label),
            include_spam_trash: false,
            max_results: Some(500),
            page_token: None,
        },
        priority,
    )
    .await
}

async fn list_starred_and_recent_sent_thread_ids(
    gmail: &Gmail,
    priority: GmailPriority,
) -> Result<(Vec<GmailThreadId>, Vec<GmailThreadId>), GmailError> {
    let starred_label = SystemLabel::Starred.label_id();
    tokio::try_join!(
        list_all_thread_ids(
            gmail,
            ThreadQuery {
                q: None,
                label_id: Some(&starred_label),
                include_spam_trash: false,
                max_results: Some(500),
                page_token: None,
            },
            priority,
        ),
        list_all_thread_ids(
            gmail,
            ThreadQuery {
                q: Some("in:sent newer_than:90d"),
                label_id: None,
                include_spam_trash: false,
                max_results: Some(500),
                page_token: None,
            },
            priority,
        ),
    )
}

async fn list_all_remote_threads(
    gmail: &Gmail,
) -> Result<HashMap<GmailThreadId, Option<HistoryId>>, GmailError> {
    let mut page_token = None;
    let mut remote_map = HashMap::new();
    loop {
        let page = gmail
            .list_threads(
                ThreadQuery {
                    q: None,
                    label_id: None,
                    include_spam_trash: true,
                    max_results: Some(500),
                    page_token: page_token.as_ref(),
                },
                GmailPriority::P2,
            )
            .await?;
        for tref in page.threads {
            remote_map.insert(tref.id, tref.history_id);
        }
        if page.next_page_token.is_none() {
            break;
        }
        page_token = page.next_page_token;
    }
    Ok(remote_map)
}

#[expect(
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    reason = "full resync compares remote thread map with local states, probes missing threads, and reconciles drafts"
)]
async fn run_full_resync(
    slot: &AccountSlot,
    gmail: &Gmail,
    cancel: &CancellationToken,
) -> Result<(), GmailError> {
    slot.set_status(SyncStatus::Syncing).await;
    let profile = gmail.get_profile(GmailPriority::P2).await?;
    let new_cursor = profile.history_id;

    refresh_labels_and_send_as(slot, gmail, GmailPriority::P2).await?;
    let remote_map = list_all_remote_threads(gmail).await?;
    let inbox_ids = list_inbox_thread_ids(gmail, GmailPriority::P2).await?;
    let (starred_ids, sent_ids) =
        list_starred_and_recent_sent_thread_ids(gmail, GmailPriority::P2).await?;
    let first_sync_scope: HashSet<GmailThreadId> = inbox_ids
        .into_iter()
        .chain(starred_ids)
        .chain(sent_ids)
        .collect();

    let local_states = slot
        .pool
        .with(|db| db.thread_sync_states(slot.account))
        .unwrap_or_default();
    let local_by_gid: HashMap<GmailThreadId, (ThreadId, Option<HistoryId>)> = local_states
        .iter()
        .map(|s| (s.gmail_id.clone(), (s.id, s.history_id)))
        .collect();

    let mut to_fetch = Vec::new();
    let mut to_clear_history = Vec::new();
    for (gtid, remote_hid) in &remote_map {
        match local_by_gid.get(gtid) {
            None if first_sync_scope.contains(gtid) => to_fetch.push(gtid.clone()),
            Some((tid, local_hid)) if local_hid.is_none() || *local_hid != *remote_hid => {
                to_clear_history.push(*tid);
                to_fetch.push(gtid.clone());
            }
            None | Some(_) => {}
        }
    }

    if !to_clear_history.is_empty() {
        let acct = slot.account;
        super::super::log_err(
            "clear changed thread history ids",
            slot.writer
                .call(StorePriority::Sync, move |tx, _state| {
                    tx.clear_thread_history_ids(acct, &to_clear_history)
                })
                .await,
        );
    }

    let mut to_delete = Vec::new();
    for s in &local_states {
        if !remote_map.contains_key(&s.gmail_id) {
            match gmail
                .get_thread(&s.gmail_id, Format::Minimal, GmailPriority::P2)
                .await
            {
                Ok(_) => to_fetch.push(s.gmail_id.clone()),
                Err(err) if err.outcome == Outcome::NotFound => to_delete.push(s.id),
                Err(err) => return Err(err),
            }
        }
    }

    if !to_delete.is_empty() {
        super::super::log_err(
            "delete vanished threads during resync",
            slot.writer
                .call(StorePriority::Sync, move |tx, state| {
                    for tid in to_delete {
                        tx.delete_thread(tid)?;
                    }
                    state.emit(Effect::RefreshBadge);
                    Ok::<_, StoreError>(())
                })
                .await,
        );
    }

    let all_read = fetch_batch_and_wait(
        slot,
        &to_fetch,
        Format::Metadata,
        FetchPriority::SyncNew,
        cancel,
    )
    .await;
    if cancel.is_cancelled() {
        return Ok(());
    }
    if !all_read {
        return Err(threads_left_unread());
    }
    for id in &to_fetch {
        slot.fetch_queue.enqueue(
            id.clone(),
            Format::Full,
            FetchPriority::SyncNew,
            false,
            None,
            &slot.activity,
        );
    }

    let draft_threads = reconcile_drafts_with_gmail(
        slot.account,
        gmail,
        &slot.pool,
        &slot.writer,
        GmailPriority::P2,
    )
    .await?;
    let fetched: HashSet<&GmailThreadId> = to_fetch.iter().collect();
    for gtid in &draft_threads {
        if !fetched.contains(gtid) {
            slot.fetch_queue.enqueue(
                gtid.clone(),
                Format::Full,
                FetchPriority::SyncNew,
                false,
                None,
                &slot.activity,
            );
        }
    }

    let acct = slot.account;
    super::super::log_err(
        "store resync history cursor",
        slot.writer
            .call(StorePriority::Sync, move |tx, _state| {
                tx.set_history(acct, new_cursor)
            })
            .await,
    );
    Ok(())
}

async fn refresh_labels_and_send_as(
    slot: &AccountSlot,
    gmail: &Gmail,
    priority: GmailPriority,
) -> Result<(), GmailError> {
    refresh_labels_guarded(slot, gmail, priority).await?;
    if let Err(err) = refresh_send_as_guarded(slot, gmail, priority).await {
        tracing::debug!(error = %err, "send-as refresh skipped");
    }
    Ok(())
}

pub(crate) async fn refresh_labels_guarded(
    slot: &AccountSlot,
    gmail: &Gmail,
    priority: GmailPriority,
) -> Result<(), GmailError> {
    let acct = slot.account;
    let start_gen = slot
        .writer
        .call(StorePriority::Sync, move |_tx, state| {
            Ok::<_, StoreError>(state.freshness.labels_gen(acct))
        })
        .await
        .unwrap_or(0);
    let labels = gmail.list_labels(priority).await?;
    let mapped: Vec<(LabelId, String, bool)> = labels
        .into_iter()
        .map(|l| (l.id, l.name, l.kind == LabelType::System))
        .collect();
    super::super::log_err(
        "sync labels",
        slot.writer
            .call(StorePriority::Sync, move |tx, state| {
                if state.freshness.labels_gen(acct) != start_gen {
                    return Ok::<_, StoreError>(());
                }
                tx.sync_labels(acct, &mapped)?;
                state.freshness.bump_labels(acct);
                Ok(())
            })
            .await,
    );
    Ok(())
}

pub(crate) async fn refresh_send_as_guarded(
    slot: &AccountSlot,
    gmail: &Gmail,
    priority: GmailPriority,
) -> Result<(), GmailError> {
    let acct = slot.account;
    let start_gen = slot
        .writer
        .call(StorePriority::Sync, move |_tx, state| {
            Ok::<_, StoreError>(state.freshness.send_as_gen(acct))
        })
        .await
        .unwrap_or(0);
    let send_as = gmail.list_send_as(priority).await?;
    let mapped_send_as: Vec<SendAsAddress> = send_as
        .into_iter()
        .map(|s| SendAsAddress {
            email: s.email,
            display_name: s.display_name,
            primary: s.is_primary,
        })
        .collect();
    let display_name = mapped_send_as
        .iter()
        .find(|s| s.primary && !s.display_name.trim().is_empty())
        .or_else(|| {
            mapped_send_as
                .iter()
                .find(|s| !s.display_name.trim().is_empty())
        })
        .map(|s| s.display_name.clone());
    super::super::log_err(
        "sync send-as addresses",
        slot.writer
            .call(StorePriority::Sync, move |tx, state| {
                if state.freshness.send_as_gen(acct) != start_gen {
                    return Ok::<_, StoreError>(());
                }
                if let Some(rec) = tx.read().account(acct)? {
                    let name = display_name.unwrap_or(rec.display_name);
                    tx.set_profile(acct, &name, &mapped_send_as)?;
                }
                state.freshness.bump_send_as(acct);
                Ok::<_, StoreError>(())
            })
            .await,
    );
    Ok(())
}

async fn list_all_thread_ids(
    gmail: &Gmail,
    base: ThreadQuery<'_>,
    priority: GmailPriority,
) -> Result<Vec<GmailThreadId>, GmailError> {
    let mut out = Vec::new();
    let mut page_token = None;
    loop {
        let mut q = base;
        q.page_token = page_token.as_ref();
        let page = gmail.list_threads(q, priority).await?;
        for t in page.threads {
            out.push(t.id);
        }
        if page.next_page_token.is_none() {
            break;
        }
        page_token = page.next_page_token;
    }
    Ok(out)
}

async fn fetch_batch_and_wait(
    slot: &AccountSlot,
    ids: &[GmailThreadId],
    format: Format,
    priority: FetchPriority,
    cancel: &CancellationToken,
) -> bool {
    if ids.is_empty() {
        return true;
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    for id in ids {
        slot.fetch_queue.enqueue(
            id.clone(),
            format,
            priority,
            false,
            Some(tx.clone()),
            &slot.activity,
        );
    }
    drop(tx);
    let mut settled = 0usize;
    tokio::select! {
        () = cancel.cancelled() => return false,
        () = async {
            while rx.recv().await.is_some() {
                settled = settled.saturating_add(1);
            }
        } => {}
    }
    settled >= ids.len()
}

fn threads_left_unread() -> GmailError {
    GmailError::new(
        Outcome::NoResponse,
        "some conversations could not be read from Gmail",
    )
}

fn spawn_contacts_backfill(slot: Arc<AccountSlot>, gmail: Arc<Gmail>, tasks: &Tasks) {
    let cancel = tasks.token();
    tasks.spawn_busy(async move {
        run_contacts_backfill(&slot, &gmail, &cancel).await;
    });
}

#[expect(
    clippy::too_many_lines,
    reason = "backfills metadata-only sent threads in memory to populate contact autocomplete sightings"
)]
async fn run_contacts_backfill(slot: &AccountSlot, gmail: &Gmail, cancel: &CancellationToken) {
    let Some(Ok(ids)) = cancel
        .run_until_cancelled(list_all_thread_ids(
            gmail,
            ThreadQuery {
                q: Some("in:sent newer_than:1y"),
                label_id: None,
                include_spam_trash: false,
                max_results: Some(250),
                page_token: None,
            },
            GmailPriority::P6,
        ))
        .await
    else {
        return;
    };

    let existing = slot
        .pool
        .with(|db| db.thread_sync_states(slot.account))
        .unwrap_or_default();
    let known: HashSet<GmailThreadId> = existing.into_iter().map(|s| s.gmail_id).collect();

    let mut sightings = Vec::new();
    for gtid in ids.into_iter().take(CONTACTS_BACKFILL_MAX) {
        if cancel.is_cancelled() || !slot.is_online() {
            break;
        }
        if known.contains(&gtid) {
            continue;
        }
        let Some(fetched) = cancel
            .run_until_cancelled(gmail.get_thread(&gtid, Format::Metadata, GmailPriority::P6))
            .await
        else {
            break;
        };
        let Ok(thread) = fetched else {
            continue;
        };
        for msg in thread.messages {
            let Some(payload) = &msg.payload else {
                continue;
            };
            let parsed = parse_message(payload);
            let labels: LabelSet = msg.label_ids.into_iter().collect();
            if labels.has(SystemLabel::Draft) {
                continue;
            }
            let from = parsed
                .headers
                .from
                .into_iter()
                .next()
                .unwrap_or_else(|| crate::domain::Addr::new(""));
            sightings.push(ContactSighting {
                gmail_msg_id: msg.id,
                date: msg
                    .internal_date
                    .or(parsed.headers.date)
                    .unwrap_or(Millis(0)),
                sent_by_me: labels.has(SystemLabel::Sent),
                is_list: parsed.headers.is_list,
                from,
                to: parsed.headers.to,
                cc: parsed.headers.cc,
            });
        }
    }

    if !sightings.is_empty() && !cancel.is_cancelled() {
        let acct = slot.account;
        super::super::log_err(
            "record contacts backfill sightings",
            slot.writer
                .call(StorePriority::Background, move |tx, _state| {
                    tx.record_contacts(acct, &sightings)
                })
                .await,
        );
    }
}
