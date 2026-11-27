pub(crate) mod actions;
pub(crate) mod attachments;
mod commands;
pub(crate) mod drafts;
pub(crate) mod events;
pub(crate) mod listings;
pub(crate) mod network;
pub(crate) mod notify;
pub(crate) mod outbox;
pub(crate) mod runtime;
pub(crate) mod snooze;
pub(crate) mod subs;
pub(crate) mod sync;
pub(crate) mod unwanted;
pub(crate) mod writer_state;

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(any(test, feature = "fake"))]
use std::time::Duration;

use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::domain::{AccountId, SettingKey, ThreadId};
use crate::gmail::{
    HttpClient, OAuthConfig, Priority as GmailPriority, QuotaConfig, Scheduler, TokenFile,
};
use crate::ipc::{CommandError, ErrorKind};
use crate::store::{Priority as StorePriority, ReadPool, StoreError, Writer};

use events::{Effect, EffectBus};
use listings::ListingManager;
use network::Network;
use notify::{NotificationTracker, refresh_badge};
use runtime::{Kick, Tasks};
use snooze::{run_snooze_loop, wake_due_snoozes};
use sync::AccountManager;
use sync::drafts::{download_carried_draft_attachments, reload_single_draft_from_gmail};

pub use drafts::DraftFiles;
#[cfg(any(test, feature = "fake"))]
pub use runtime::ManualClock;
pub use runtime::{Activity, Busy, Clock, ClockSleep};
pub use subs::diff_list;
pub use writer_state::{ChannelSink, WriterState};

const READ_POOL_SIZE: usize = 4;

pub(crate) fn log_err<T, E: std::fmt::Display>(what: &'static str, res: Result<T, E>) {
    if let Err(err) = res {
        tracing::warn!(error = %err, "failed to {what}");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub account: String,
    pub thread: Option<ThreadId>,
    pub title: String,
    pub body: String,
}

pub trait Platform: Send + Sync {
    fn open_url(&self, url: &str) -> Result<(), CoreError>;
    fn open_file(&self, path: &Path) -> Result<(), CoreError>;
    fn notify(&self, notification: Notification);
    fn set_badge(&self, count: u32);
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct CoreError {
    pub kind: ErrorKind,
    pub message: String,
    pub internal: bool,
}

impl CoreError {
    #[must_use]
    pub fn other(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Other,
            message: message.into(),
            internal: false,
        }
    }

    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Other,
            message: message.into(),
            internal: true,
        }
    }

    #[must_use]
    pub fn sign_in_cancelled() -> Self {
        Self {
            kind: ErrorKind::SignInCancelled,
            message: "Sign-in was cancelled".to_owned(),
            internal: false,
        }
    }

    #[must_use]
    pub fn draft_gone() -> Self {
        Self {
            kind: ErrorKind::DraftGone,
            message: "Draft was sent or deleted elsewhere".to_owned(),
            internal: false,
        }
    }

    #[must_use]
    pub fn into_command_error(self) -> CommandError {
        let message = if self.internal {
            tracing::warn!(detail = %self.message, "redacting internal command error");
            "Something went wrong inside Supermonkey".to_owned()
        } else {
            self.message
        };
        CommandError {
            kind: self.kind,
            message,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CoreConfig {
    pub data_dir: PathBuf,
    pub gmail_base_url: String,
    pub oauth: OAuthConfig,
    pub quota: QuotaConfig,
    pub clock: Clock,
}

pub struct Core {
    pub(crate) data_dir: PathBuf,
    pub(crate) http: HttpClient,
    pub(crate) pool: Arc<ReadPool>,
    pub(crate) writer: Writer<WriterState>,
    pub(crate) clock: Clock,
    pub(crate) activity: Activity,
    pub(crate) tasks: Tasks,
    pub(crate) files: DraftFiles,
    pub(crate) accounts: AccountManager,
    pub(crate) listings: ListingManager,
    pub(crate) network: Network,
    pub(crate) notifications: NotificationTracker,
    pub(crate) snooze_kick: Kick,
    pub(crate) platform: Box<dyn Platform>,
    channel: Arc<Mutex<Option<ChannelSink>>>,
    effects_rx: Mutex<Option<mpsc::UnboundedReceiver<(Effect, Busy)>>>,
}

impl std::fmt::Debug for Core {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Core")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

impl Core {
    #[expect(
        clippy::too_many_lines,
        reason = "wires store, read pool, scheduler, accounts, listings, and network monitor"
    )]
    pub fn open(config: CoreConfig, platform: Box<dyn Platform>) -> Result<Arc<Self>, CoreError> {
        let http = HttpClient::new().map_err(|e| CoreError::internal(e.to_string()))?;
        let db_path = config.data_dir.join("mail.db");
        import_earlier_database(&config.data_dir, &db_path);
        let tokens = Arc::new(TokenFile::new(config.data_dir.join("tokens.json")));
        let files = DraftFiles::new(&config.data_dir);
        let activity = Activity::new();
        let (bus, effects_rx) = EffectBus::new(activity.clone());
        let channel: Arc<Mutex<Option<ChannelSink>>> = Arc::new(Mutex::new(None));

        let initial_conn =
            crate::store::open_writer(&db_path).map_err(|e| CoreError::internal(e.to_string()))?;
        drop(initial_conn);

        let pool_size = NonZeroUsize::new(READ_POOL_SIZE).unwrap_or(NonZeroUsize::MIN);
        let pool = Arc::new(
            ReadPool::open(&db_path, pool_size).map_err(|e| CoreError::internal(e.to_string()))?,
        );
        let (ack, rev) = pool
            .with(|db| Ok::<_, StoreError>((db.last_action()?, db.rev()?)))
            .map_err(|e| CoreError::internal(e.to_string()))?;

        let writer_state = WriterState::new(
            ack,
            rev,
            true,
            config.clock.clone(),
            bus,
            Arc::clone(&channel),
        );
        let writer = Writer::start(
            &db_path,
            writer_state,
            WriterState::on_commit,
            WriterState::on_rollback,
        )
        .map_err(|e| CoreError::internal(e.to_string()))?;

        let tasks = Tasks::new(activity.clone());
        let scheduler = Scheduler::new(config.quota);
        let network = Network::new(
            &config.gmail_base_url,
            http.clone(),
            writer.clone(),
            config.clock.clone(),
            activity.clone(),
        );

        let accounts = AccountManager::new(
            http.clone(),
            config.gmail_base_url.clone(),
            config.oauth,
            tokens,
            scheduler,
            Arc::clone(&pool),
            writer.clone(),
            config.clock.clone(),
            network.clone(),
            activity.clone(),
            files.clone(),
            tasks.child(),
        );
        let listings = ListingManager::new(
            accounts.clone(),
            Arc::clone(&pool),
            writer.clone(),
            config.clock.clone(),
            network.clone(),
            activity.clone(),
            tasks.child(),
        );

        Ok(Arc::new(Self {
            data_dir: config.data_dir,
            http,
            pool,
            writer,
            clock: config.clock,
            activity,
            tasks,
            files,
            accounts,
            listings,
            network,
            notifications: NotificationTracker::new(),
            snooze_kick: Kick::new(),
            platform,
            channel,
            effects_rx: Mutex::new(Some(effects_rx)),
        }))
    }

    pub fn set_channel(&self, sink: Option<ChannelSink>) {
        *self.channel.lock() = sink;
    }

    async fn sweep_crashed_drafts(&self) {
        let now = self.clock.now();
        log_err(
            "sweep untouched local drafts at launch",
            self.writer
                .call(StorePriority::Sync, move |tx, state| {
                    let accounts = tx.read().accounts()?;
                    for acct in accounts {
                        let list = tx.read().drafts_for_account(acct.id)?;
                        for mut d in list {
                            if d.in_outbox || d.lifecycle.is_kept() {
                                continue;
                            }
                            if drafts::draft_is_untouched(&d) {
                                tx.delete_draft(&d.id)?;
                                if let Some(remote_id) = d.lifecycle.gmail_draft_id {
                                    let payload = writer_state::DraftOpPayloadRef {
                                        draft_id: &d.id,
                                        gmail_draft_id: Some(&remote_id),
                                    };
                                    tx.queue_op(&crate::store::NewOp {
                                        account: acct.id,
                                        action: None,
                                        kind: crate::domain::OpKind::DraftDelete,
                                        draft_id: Some(&d.id),
                                        gmail_draft_id: Some(&remote_id),
                                        payload: &payload,
                                    })?;
                                    state.emit(Effect::KickOps(acct.id));
                                }
                                state.emit(Effect::RemoveDraftFiles(d.id));
                            } else {
                                d.lifecycle.keep();
                                tx.put_draft(&d.as_write())?;
                            }
                        }
                    }
                    for mut ob in tx.read().outbox_all()? {
                        if ob.item.state == crate::domain::OutboxState::Sent {
                            tx.delete_undo_for_outbox(ob.item.account, ob.item.id)?;
                            tx.delete_outbox(ob.item.id)?;
                            tx.delete_draft(&ob.item.draft)?;
                            state.emit(Effect::RemoveDraftFiles(ob.item.draft));
                        } else if ob.item.state == crate::domain::OutboxState::Waiting
                            && ob.item.send_after < now
                            && !ob.item.delayed
                        {
                            ob.item.delayed = true;
                            tx.update_outbox(&ob.item)?;
                        }
                    }
                    Ok::<_, StoreError>(())
                })
                .await,
        );
    }

    pub async fn start(self: &Arc<Self>) -> Result<(), CoreError> {
        self.sweep_crashed_drafts().await;
        log_err(
            "wake due snoozes at launch",
            wake_due_snoozes(&self.writer, &self.clock).await,
        );
        #[expect(
            clippy::redundant_closure_for_method_calls,
            reason = "Db carries a lifetime so a closure is needed for ReadPool::with HRTB"
        )]
        if let Ok(live) = self.pool.with(|db| db.all_draft_ids()) {
            self.files.sweep_orphans(&live);
        }
        refresh_badge(&self.pool, self.platform.as_ref());

        let effects_rx = self.effects_rx.lock().take();
        if let Some(rx) = effects_rx {
            let core = Arc::clone(self);
            let cancel = self.tasks.token();
            self.tasks.spawn(async move {
                run_effect_loop(core, rx, cancel).await;
            });
        }

        let snooze_cancel = self.tasks.token();
        self.tasks.spawn(run_snooze_loop(
            Arc::clone(&self.pool),
            self.writer.clone(),
            self.clock.clone(),
            self.snooze_kick.clone(),
            self.activity.clone(),
            snooze_cancel,
        ));

        self.network.spawn_probe(&self.tasks);
        self.accounts
            .start_existing()
            .await
            .map_err(|e| CoreError::internal(e.to_string()))?;
        Ok(())
    }

    pub async fn stop(&self) {
        self.listings.reset();
        self.tasks.stop().await;
        log_err("stop database writer", self.writer.stop().await);
    }

    pub fn on_focus(&self, focused: bool) -> Option<(AccountId, Option<ThreadId>)> {
        let open_target = self.notifications.set_focused(focused, &self.clock);
        if focused {
            self.accounts.on_focus();
        }
        open_target
    }

    #[must_use]
    pub const fn clock(&self) -> &Clock {
        &self.clock
    }

    #[must_use]
    pub fn enter_busy(&self) -> Busy {
        self.activity.enter()
    }

    #[must_use]
    pub const fn draft_files(&self) -> &DraftFiles {
        &self.files
    }

    pub fn poll_all(&self) {
        self.accounts.poll_all();
    }

    #[cfg(any(test, feature = "fake"))]
    pub async fn probe_network(&self) {
        self.network.probe_now().await;
    }

    #[cfg(any(test, feature = "fake"))]
    pub fn wake_retrying(&self) {
        self.accounts.wake_retrying();
    }

    #[cfg(any(test, feature = "fake"))]
    #[must_use]
    pub const fn activity(&self) -> &Activity {
        &self.activity
    }

    #[cfg(any(test, feature = "fake"))]
    pub async fn wait_idle(&self, timeout: Duration) -> bool {
        self.activity.wait_idle(timeout).await
    }

    #[cfg(any(test, feature = "fake"))]
    #[must_use]
    pub fn read_pool(&self) -> &ReadPool {
        &self.pool
    }

    #[cfg(any(test, feature = "fake"))]
    #[must_use]
    pub const fn writer(&self) -> &Writer<WriterState> {
        &self.writer
    }
}

fn import_earlier_database(data_dir: &Path, db_path: &Path) {
    let earlier = data_dir.join(crate::store::LEGACY_DB_FILE);
    if !db_path.exists() && earlier.exists() {
        log_err(
            "import the database of an earlier version",
            crate::store::import_legacy(&earlier, db_path),
        );
    }
}

async fn run_effect_loop(
    core: Arc<Core>,
    mut rx: mpsc::UnboundedReceiver<(Effect, Busy)>,
    cancel: tokio_util::sync::CancellationToken,
) {
    loop {
        let item = tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            next = rx.recv() => match next {
                Some(i) => i,
                None => return,
            }
        };
        let (effect, busy) = item;
        handle_effect(&core, effect).await;
        drop(busy);
    }
}

async fn handle_effect(core: &Arc<Core>, effect: Effect) {
    match effect {
        Effect::KickOps(account) => core.accounts.kick_ops(account),
        Effect::KickOutbox(account) => core.accounts.kick_outbox(account),
        Effect::KickSnooze => core.snooze_kick.wake(&core.activity),
        Effect::RefreshBadge => refresh_badge(&core.pool, core.platform.as_ref()),
        Effect::OpenConversation { account, thread } => {
            core.accounts.open_conversation(account, thread);
        }
        Effect::SubscribeList { sub, account, key } => {
            core.listings.subscribe(sub, account, key);
        }
        Effect::MoreResults { sub } => core.listings.more_results(&sub),
        Effect::Unsubscribe(sub) => core.listings.unsubscribe(&sub),
        Effect::CancelAccountListings(account) => core.listings.cancel_account(account),
        Effect::ResetSubs => core.listings.reset(),
        Effect::FetchSubscribedBodies => fetch_subscribed_bodies(core).await,
        Effect::NetworkChanged { online: true } => {
            core.accounts.on_online();
            fetch_subscribed_bodies(core).await;
        }
        Effect::NetworkChanged { online: false } => core.accounts.on_offline(),
        Effect::RemoveAccount { account, email } => {
            core.accounts.remove_account(account, email);
        }
        Effect::RemoveDraftFiles(draft) => core.files.remove_draft_dir(&draft),
        Effect::RemoveAttachmentFiles(paths) => core.files.remove_attachment_files(&paths),
        Effect::EnsureDraftAttachments { account, draft } => {
            spawn_ensure_attachments(core, account, draft);
        }
        Effect::ReloadDraftFromGmail {
            account,
            draft,
            gmail_draft_id,
        } => spawn_reload_draft(core, account, draft, gmail_draft_id),
        Effect::NotifyNewMail {
            account,
            email,
            items,
        } => dispatch_new_mail_notification(core, account, &email, &items).await,
    }
}

fn spawn_ensure_attachments(core: &Arc<Core>, account: AccountId, draft: crate::domain::DraftId) {
    let core_c = Arc::clone(core);
    let cancel = core.tasks.token();
    core.tasks.spawn_busy(async move {
        if let Some((_, gmail)) = core_c.accounts.gmail_for(account)
            && let Some(res) = cancel
                .run_until_cancelled(download_carried_draft_attachments(
                    account,
                    &draft,
                    &gmail,
                    &core_c.pool,
                    &core_c.writer,
                    &core_c.files,
                    GmailPriority::P1,
                ))
                .await
        {
            log_err("download carried draft attachments", res);
        }
    });
}

fn spawn_reload_draft(
    core: &Arc<Core>,
    account: AccountId,
    draft: crate::domain::DraftId,
    gmail_draft_id: crate::domain::GmailDraftId,
) {
    let core_c = Arc::clone(core);
    let cancel = core.tasks.token();
    core.tasks.spawn_busy(async move {
        if let Some((_, gmail)) = core_c.accounts.gmail_for(account) {
            cancel
                .run_until_cancelled(reload_single_draft_from_gmail(
                    account,
                    draft,
                    &gmail_draft_id,
                    &gmail,
                    &core_c.pool,
                    &core_c.writer,
                    GmailPriority::P2,
                ))
                .await;
        }
    });
}

async fn fetch_subscribed_bodies(core: &Core) {
    let threads: Vec<ThreadId> = core
        .writer
        .call(StorePriority::Sync, |_tx, state| {
            Ok::<_, StoreError>(state.registry.subscribed_threads().into_iter().collect())
        })
        .await
        .unwrap_or_default();
    for tid in threads {
        if let Ok(Some(row)) = core.pool.with(|db| db.thread_row(tid)) {
            core.accounts.open_conversation(row.account, tid);
        }
    }
}

async fn dispatch_new_mail_notification(
    core: &Core,
    account: AccountId,
    email: &str,
    items: &[events::NewMailAlert],
) {
    let muted = core
        .pool
        .with(|db| db.setting_is_on(Some(account), SettingKey::Muted))
        .unwrap_or(false);
    let open_threads: HashSet<ThreadId> = core
        .writer
        .call(StorePriority::Sync, |_tx, state| {
            Ok::<_, StoreError>(state.registry.open_threads())
        })
        .await
        .unwrap_or_default();
    core.notifications.notify_new_mail(
        account,
        email,
        items,
        &open_threads,
        muted,
        &core.clock,
        core.platform.as_ref(),
    );
}
