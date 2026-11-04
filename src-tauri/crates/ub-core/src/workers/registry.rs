//! The account workers that run (ARCHITECTURE.md, sections 4 and 8.1): which
//! accounts sync and with which grant. Starting, replacing, signing out, and
//! forgetting them happens here, so no two workers of one account ever run
//! at once.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use tokio::sync::{RwLock, mpsc};
use ub_domain::{AccountId, DraftId, ThreadId};
use ub_gmail::{Gmail, HttpClient, RefreshToken, Scheduler, TokenStore, Tokens};
use ub_store::Priority;

use super::account::{AccountWorker, SignOutSender, WorkerDeps, WorkerId};
use super::files::{attachments_dir, draft_dir};
use super::ops::OpsLane;
use crate::activity::{Tasks, Tracked, parked};
use crate::writer::CoreWriter;
use crate::{Error, GmailConfig};

/// Every running account worker, and what starting one takes.
pub(crate) struct AccountWorkers {
    deps: Arc<WorkerDeps>,
    gmail: GmailConfig,
    http: HttpClient,
    scheduler: Scheduler,
    tokens: Arc<dyn TokenStore>,
    tasks: Tasks,
    live: Arc<Live>,
    /// Each account's op lane. It outlives the account's workers, so an op
    /// a replaced worker had sent lands before the next one replays.
    lanes: Mutex<HashMap<AccountId, Arc<OpsLane>>>,
    sign_outs: SignOutSender,
    next_id: AtomicU64,
    /// Workers start only once the core has launched.
    launched: AtomicBool,
    /// Held (shared) by each removed account's clean-up. A sign-in waits for
    /// them first: a revoke landing after it would end the new grant too.
    removals: Arc<RwLock<()>>,
}

/// The running workers, by account.
struct Live {
    workers: Mutex<HashMap<AccountId, AccountWorker>>,
    writer: CoreWriter,
}

impl Live {
    /// Takes the account's worker out if it is `id`: a report from a
    /// worker already replaced changes nothing.
    fn take_if(&self, account: AccountId, id: WorkerId) -> Option<AccountWorker> {
        let mut workers = self.workers.lock();
        if workers.get(&account).is_some_and(|w| w.id() == id) {
            workers.remove(&account)
        } else {
            None
        }
    }
}

impl AccountWorkers {
    pub(crate) fn new(
        deps: Arc<WorkerDeps>,
        gmail: GmailConfig,
        http: HttpClient,
        tokens: Arc<dyn TokenStore>,
        tasks: Tasks,
    ) -> Arc<Self> {
        let live = Arc::new(Live {
            workers: Mutex::new(HashMap::new()),
            writer: deps.writer.clone(),
        });
        let (sign_outs, reports) = mpsc::unbounded_channel();
        tasks.spawn("sign outs", sign_out_reports(Arc::clone(&live), reports));
        Arc::new(Self {
            scheduler: Scheduler::new(gmail.quota),
            deps,
            gmail,
            http,
            tokens,
            tasks,
            live,
            lanes: Mutex::default(),
            sign_outs,
            next_id: AtomicU64::new(0),
            launched: AtomicBool::new(false),
            removals: Arc::default(),
        })
    }

    /// The Gmail endpoints and OAuth client.
    pub(crate) fn gmail_config(&self) -> &GmailConfig {
        &self.gmail
    }

    /// The shared HTTP client.
    pub(crate) fn http(&self) -> &HttpClient {
        &self.http
    }

    /// The per-account request schedulers.
    pub(crate) fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    /// Where refresh tokens live.
    pub(crate) fn tokens(&self) -> &Arc<dyn TokenStore> {
        &self.tokens
    }

    /// The writer.
    pub(crate) fn writer(&self) -> &CoreWriter {
        &self.deps.writer
    }

    /// The folder of a draft's added files.
    pub(crate) fn draft_dir(&self, draft: &DraftId) -> PathBuf {
        draft_dir(&self.deps.data_dir, draft)
    }

    /// Deletes a draft's added files: its row is gone.
    pub(crate) fn delete_draft_files(&self, draft: &DraftId) {
        let dir = self.draft_dir(draft);
        self.tasks.spawn("delete draft files", async move {
            let deleted =
                tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                })
                .await;
            match deleted {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(error = %e, "couldn't delete a draft's files"),
                Err(e) => tracing::warn!(error = %e, "deleting a draft's files failed"),
            }
        });
    }

    /// The account's running worker.
    pub(crate) fn get(&self, account: AccountId) -> Option<AccountWorker> {
        self.live.workers.lock().get(&account).cloned()
    }

    fn all(&self) -> Vec<AccountWorker> {
        self.live.workers.lock().values().cloned().collect()
    }

    /// Starts a worker for every signed-in account (SPEC Accounts). An
    /// account with no stored token shows Sign in again. From now on, a
    /// registered account starts its worker at once.
    pub(crate) fn launch(self: &Arc<Self>) {
        if self.launched.swap(true, Ordering::SeqCst) {
            return;
        }
        let workers = Arc::clone(self);
        self.tasks.spawn("launch accounts", async move {
            let pool = Arc::clone(&workers.deps.pool);
            let accounts =
                tokio::task::spawn_blocking(move || pool.with(|r| r.accounts().all())).await;
            let accounts = match accounts {
                Ok(Ok(accounts)) => accounts,
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "couldn't read the accounts at launch");
                    return;
                }
                Err(e) => {
                    tracing::error!(error = %e, "the launch read failed");
                    return;
                }
            };
            for record in accounts.into_iter().filter(|a| !a.needs_signin) {
                if let Err(e) = workers.start_stored(record.id, &record.email).await {
                    tracing::warn!(account = record.id.0, error = %e, "couldn't start the account");
                }
            }
        });
    }

    /// An account was added or signed in again with the refresh token in
    /// the store: starts its worker, if the core has launched.
    pub(crate) async fn registered(&self, account: AccountId, email: &str) -> Result<(), Error> {
        if self.launched.load(Ordering::SeqCst) {
            self.start_stored(account, email).await?;
        }
        Ok(())
    }

    /// A sign-in finished with `tokens`: starts the account's worker on
    /// them, if the core has launched.
    pub(crate) fn signed_in(
        &self,
        account: AccountId,
        email: &str,
        tokens: Arc<Tokens>,
    ) -> Result<(), Error> {
        if self.launched.load(Ordering::SeqCst) {
            self.start(account, email, tokens)?;
        }
        Ok(())
    }

    /// Starts the account on its stored refresh token, or marks it signed
    /// out when there is none.
    async fn start_stored(&self, account: AccountId, email: &str) -> Result<(), Error> {
        match self.load_token(email).await {
            Ok(Some(token)) => {
                let tokens = Tokens::new(self.gmail.oauth.clone(), self.http.clone(), token);
                self.start(account, email, Arc::new(tokens))
            }
            Ok(None) => {
                tracing::info!(
                    account = account.0,
                    "no refresh token; the account needs a sign-in"
                );
                self.mark_signed_out(account);
                Ok(())
            }
            Err(e) => {
                tracing::warn!(account = account.0, error = %e, "couldn't read the refresh token; the account needs a sign-in");
                self.mark_signed_out(account);
                Ok(())
            }
        }
    }

    async fn load_token(&self, email: &str) -> Result<Option<RefreshToken>, Error> {
        let (tokens, email) = (Arc::clone(&self.tokens), email.to_owned());
        Ok(tokio::task::spawn_blocking(move || tokens.load(&email)).await??)
    }

    /// Starts the account's worker on `tokens`. A worker it already has is
    /// replaced: the new one's tasks start once the old one's have ended.
    fn start(&self, account: AccountId, email: &str, tokens: Arc<Tokens>) -> Result<(), Error> {
        let gmail = Gmail::new(
            &self.gmail.base_url,
            self.http.clone(),
            tokens,
            self.scheduler.account(email),
        )
        .map_err(|e| Error::Invalid(e.to_string()))?;
        let id = WorkerId(self.next_id.fetch_add(1, Ordering::SeqCst));
        let lane = Arc::clone(
            self.lanes
                .lock()
                .entry(account)
                .or_insert_with(|| OpsLane::new(self.tasks.child())),
        );
        let mut workers = self.live.workers.lock();
        let old = workers.remove(&account);
        if let Some(old) = &old {
            old.cancel();
        }
        let worker = AccountWorker::spawn(
            Arc::clone(&self.deps),
            account,
            id,
            gmail,
            self.tasks.child(),
            lane,
            self.sign_outs.clone(),
            old,
        );
        workers.insert(account, worker);
        drop(workers);
        tracing::info!(account = account.0, worker = id.0, "account worker started");
        Ok(())
    }

    fn mark_signed_out(&self, account: AccountId) {
        mark_signed_out(&self.deps.writer, account);
    }

    /// Polls every account now.
    pub(crate) fn poll_all(&self) {
        for worker in self.all() {
            worker.poll_now();
        }
    }

    /// Reads every account's `sendAs` and labels again, and polls.
    pub(crate) fn refresh_all(&self) {
        for worker in self.all() {
            worker.poll_now();
            worker.refresh_profile();
        }
    }

    /// Stops the read of `thread`'s bodies for a conversation, in whichever
    /// account has it: no subscription shows it any more.
    pub(crate) fn release_opened(&self, thread: ThreadId) {
        for worker in self.all() {
            worker.release_opened(thread);
        }
    }

    /// Ends every wait for the network and every back-off now.
    pub(crate) fn retry_now(&self) {
        self.deps.network.retry_now();
    }

    /// Waits until no removed account is still being cleaned up.
    pub(crate) async fn removals_done(&self) {
        // A removal's clean-up counts as busy on its own.
        drop(parked(self.removals.write()).await);
    }

    /// An account was removed (its rows are gone): stops its worker, revokes
    /// and deletes its refresh token, and deletes its files.
    pub(crate) async fn forget(self: &Arc<Self>, account: AccountId, email: String) {
        let removal = Arc::clone(&self.removals).read_owned().await;
        let workers = Arc::clone(self);
        self.tasks.spawn("forget account", async move {
            let _removal = removal;
            let worker = workers.live.workers.lock().remove(&account);
            if let Some(worker) = worker {
                worker.stop().await;
            }
            let lane = workers.lanes.lock().remove(&account);
            if let Some(lane) = lane {
                lane.stop().await;
            }
            match workers.load_token(&email).await {
                Ok(Some(token)) => {
                    if let Err(e) =
                        ub_gmail::revoke(&workers.gmail.oauth, &workers.http, &token).await
                    {
                        tracing::warn!(account = account.0, outcome = %e.outcome(), "couldn't revoke a removed account's grant");
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(account = account.0, error = %e, "couldn't read a removed account's token");
                }
            }
            let tokens = Arc::clone(&workers.tokens);
            let dir = attachments_dir(&workers.deps.data_dir, account);
            let cleaned = tokio::task::spawn_blocking(move || {
                if let Err(e) = tokens.delete(&email) {
                    tracing::warn!(account = account.0, error = %e, "couldn't delete a removed account's token");
                }
                match std::fs::remove_dir_all(&dir) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        tracing::warn!(account = account.0, error = %e, "couldn't delete a removed account's files");
                    }
                    _ => {}
                }
            })
            .await;
            if let Err(e) = cleaned {
                tracing::warn!(account = account.0, error = %e, "the clean-up of a removed account failed");
            }
            tracing::info!(account = account.0, "account forgotten");
        });
    }
}

/// Marks the account signed out: it shows Sign in again (SPEC Accounts).
fn mark_signed_out(writer: &CoreWriter, account: AccountId) {
    writer.queue(
        Priority::Sync,
        "mark an account signed out",
        move |tx, _| {
            if tx.read().accounts().get(account)?.is_some() {
                tx.accounts().set_needs_signin(account, true)?;
            }
            Ok(())
        },
    );
}

/// Handles workers' reports that Google refused their grant: only the
/// account's current worker signs it out and stops.
async fn sign_out_reports(
    live: Arc<Live>,
    mut reports: mpsc::UnboundedReceiver<Tracked<(AccountId, WorkerId)>>,
) {
    while let Some(report) = parked(reports.recv()).await {
        let ((account, id), _handling) = report.into_parts();
        let Some(worker) = live.take_if(account, id) else {
            tracing::debug!(
                account = account.0,
                "a replaced worker's grant was refused; ignoring it"
            );
            continue;
        };
        worker.cancel();
        mark_signed_out(&live.writer, account);
    }
}
