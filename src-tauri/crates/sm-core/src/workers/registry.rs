use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use sm_domain::{AccountId, ThreadId};
use sm_gmail::{Gmail, HttpClient, RefreshToken, Scheduler, TokenFile, Tokens};
use sm_store::{AccountRecord, Priority};
use tokio::sync::{mpsc, watch};

use super::account::{AccountWorker, SignOutSender, WorkerDeps, WorkerId, WorkerParts};
use super::files::{ATTACHMENTS_DIR, attachments_dir, delete_folders, folders};
use super::grants::{Grant, Grants};
use super::ops::OpsLane;
use crate::activity::{Tasks, Tracked, parked};
use crate::read::read;
use crate::writer::CoreWriter;
use crate::{Error, GmailConfig};

pub(crate) struct AccountWorkers {
    deps: Arc<WorkerDeps>,
    gmail: GmailConfig,
    http: HttpClient,
    scheduler: Scheduler,
    tokens: Arc<TokenFile>,
    tasks: Tasks,
    live: Arc<Live>,
    lanes: Mutex<HashMap<AccountId, Arc<OpsLane>>>,
    sign_outs: SignOutSender,
    next_id: AtomicU64,
    launched: AtomicBool,
    removals: watch::Sender<usize>,
}

struct Live {
    workers: Mutex<HashMap<AccountId, AccountWorker>>,
    writer: CoreWriter,
    grants: Arc<Grants>,
}

impl Live {
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
        tokens: Arc<TokenFile>,
        tasks: Tasks,
    ) -> Arc<Self> {
        let live = Arc::new(Live {
            workers: Mutex::new(HashMap::new()),
            writer: deps.writer.clone(),
            grants: Arc::default(),
        });
        let (sign_outs, reports) = mpsc::unbounded_channel();
        tasks.spawn("sign outs", sign_out_reports(Arc::clone(&live), reports));
        let scheduler = Scheduler::new(gmail.quota);
        Arc::new(Self {
            deps,
            gmail,
            http,
            scheduler,
            tokens,
            tasks,
            live,
            lanes: Mutex::default(),
            sign_outs,
            next_id: AtomicU64::new(0),
            launched: AtomicBool::new(false),
            removals: watch::Sender::new(0),
        })
    }

    pub(crate) const fn gmail_config(&self) -> &GmailConfig {
        &self.gmail
    }

    pub(crate) const fn http(&self) -> &HttpClient {
        &self.http
    }

    pub(crate) const fn tokens(&self) -> &Arc<TokenFile> {
        &self.tokens
    }

    pub(crate) fn writer(&self) -> &CoreWriter {
        &self.deps.writer
    }

    pub(crate) fn grants(&self) -> &Arc<Grants> {
        &self.live.grants
    }

    pub(crate) fn get(&self, account: AccountId) -> Option<AccountWorker> {
        self.live.workers.lock().get(&account).cloned()
    }

    fn all(&self) -> Vec<AccountWorker> {
        self.live.workers.lock().values().cloned().collect()
    }

    pub(crate) fn launch(self: &Arc<Self>) {
        if self.launched.swap(true, Ordering::SeqCst) {
            return;
        }
        let workers = Arc::clone(self);
        self.tasks.spawn("launch accounts", async move {
            let accounts = match read(&workers.deps.pool, |r| r.accounts().all()).await {
                Ok(accounts) => accounts,
                Err(e) => {
                    tracing::error!(error = %e, "couldn't read the accounts at launch");
                    return;
                }
            };
            workers.sweep(&accounts).await;
            workers.deps.draft_files.sweep(&workers.deps.pool).await;
            for record in accounts.into_iter().filter(|a| !a.needs_signin) {
                let grant = workers.live.grants.current(record.id);
                if let Err(e) = workers.start_stored(record.id, &record.email, grant).await {
                    tracing::warn!(account = record.id.0, error = %e, "couldn't start the account");
                }
            }
        });
    }

    async fn sweep(&self, accounts: &[AccountRecord]) {
        let emails: HashSet<String> = accounts.iter().map(|a| a.email.clone()).collect();
        let ids: HashSet<String> = accounts.iter().map(|a| a.id.0.to_string()).collect();
        let tokens = Arc::clone(&self.tokens);
        let dir = self.deps.data_dir.join(ATTACHMENTS_DIR);
        let swept = tokio::task::spawn_blocking(move || {
            sweep_tokens(&tokens, &emails);
            delete_folders(&dir, folders(&dir), &ids);
        })
        .await;
        if let Err(e) = swept {
            tracing::warn!(error = %e, "the launch clean-up failed");
        }
    }

    pub(crate) async fn registered(
        &self,
        account: AccountId,
        email: &str,
        grant: Grant,
    ) -> Result<(), Error> {
        if self.launched.load(Ordering::SeqCst) {
            self.start_stored(account, email, grant).await?;
        }
        Ok(())
    }

    pub(crate) fn signed_in(
        &self,
        account: AccountId,
        email: &str,
        tokens: Arc<Tokens>,
        grant: Grant,
    ) -> Result<(), Error> {
        if self.launched.load(Ordering::SeqCst) {
            self.start(account, email, tokens, grant)?;
        }
        Ok(())
    }

    async fn start_stored(
        &self,
        account: AccountId,
        email: &str,
        grant: Grant,
    ) -> Result<(), Error> {
        match self.load_token(email).await {
            Ok(Some(token)) => {
                let tokens = Tokens::new(self.gmail.oauth.clone(), self.http.clone(), token);
                self.start(account, email, Arc::new(tokens), grant)
            }
            Ok(None) => {
                tracing::info!(
                    account = account.0,
                    "no refresh token; the account needs a sign-in"
                );
                self.mark_signed_out(account, grant);
                Ok(())
            }
            Err(e) => {
                self.token_unreadable(account, grant, &e);
                Ok(())
            }
        }
    }

    fn token_unreadable(&self, account: AccountId, grant: Grant, e: &Error) {
        tracing::warn!(account = account.0, error = %e, "couldn't read the refresh token; the account needs a sign-in");
        self.mark_signed_out(account, grant);
    }

    async fn load_token(&self, email: &str) -> Result<Option<RefreshToken>, Error> {
        let (tokens, email) = (Arc::clone(&self.tokens), email.to_owned());
        Ok(tokio::task::spawn_blocking(move || tokens.load(&email)).await??)
    }

    fn start(
        &self,
        account: AccountId,
        email: &str,
        tokens: Arc<Tokens>,
        grant: Grant,
    ) -> Result<(), Error> {
        let gmail = Gmail::new(
            &self.gmail.base_url,
            self.http.clone(),
            tokens,
            self.scheduler.account(email),
        )?;
        let id = WorkerId(self.next_id.fetch_add(1, Ordering::SeqCst));
        let lane = Arc::clone(
            self.lanes
                .lock()
                .entry(account)
                .or_insert_with(|| OpsLane::new(self.tasks.child())),
        );
        let worker = AccountWorker::new(
            WorkerParts {
                deps: Arc::clone(&self.deps),
                account,
                id,
                gmail,
                tasks: self.tasks.child(),
                lane,
                sign_out: self.sign_outs.clone(),
            },
            grant,
        );
        let old = self.live.workers.lock().insert(account, worker.clone());
        if let Some(old) = &old {
            old.cancel();
        }
        worker.start(old);
        tracing::info!(account = account.0, worker = id.0, "account worker started");
        Ok(())
    }

    pub(crate) fn mark_signed_out(&self, account: AccountId, grant: Grant) {
        mark_signed_out(&self.live, account, grant);
    }

    pub(crate) fn poll_all(&self) {
        for worker in self.all() {
            worker.poll_now();
        }
    }

    pub(crate) fn refresh_all(&self) {
        for worker in self.all() {
            worker.poll_now();
            worker.refresh_profile();
        }
    }

    pub(crate) fn release_opened(&self, thread: ThreadId) {
        for worker in self.all() {
            worker.release_opened(thread);
        }
    }

    pub(crate) fn retry_now(&self) {
        self.deps.network.retry_now();
    }

    pub(crate) async fn removals_done(&self) {
        let mut removals = self.removals.subscribe();
        if parked(removals.wait_for(|&n| n == 0)).await.is_err() {
            tracing::debug!("the removal count is gone; nothing to wait for");
        }
    }

    pub(crate) fn forget(self: &Arc<Self>, account: AccountId, email: String) {
        let removal = Removal::start(&self.removals);
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
            let token = match workers.load_token(&email).await {
                Ok(token) => token,
                Err(e) => {
                    tracing::warn!(account = account.0, error = %e, "couldn't read a removed account's token");
                    None
                }
            };
            let tokens = Arc::clone(&workers.tokens);
            let dir = attachments_dir(&workers.deps.data_dir, account);
            let gone = email.clone();
            let cleaned = tokio::task::spawn_blocking(move || {
                delete_removed_token(&tokens, &gone, account);
                delete_removed_files(&dir, account);
            })
            .await;
            if let Err(e) = cleaned {
                tracing::warn!(account = account.0, error = %e, "the clean-up of a removed account failed");
            }
            if let Some(token) = token
                && let Err(e) = sm_gmail::revoke(&workers.gmail.oauth, &workers.http, &token).await
            {
                tracing::warn!(account = account.0, outcome = %e.outcome(), "couldn't revoke a removed account's grant");
            }
            workers.scheduler.forget(&email);
            tracing::info!(account = account.0, "account forgotten");
        });
    }
}

fn delete_removed_token(tokens: &TokenFile, email: &str, account: AccountId) {
    if let Err(e) = tokens.delete(email) {
        tracing::warn!(account = account.0, error = %e, "couldn't delete a removed account's token");
    }
}

fn delete_removed_files(dir: &Path, account: AccountId) {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!(account = account.0, error = %e, "couldn't delete a removed account's files");
        }
        _ => {}
    }
}

fn sweep_tokens(tokens: &TokenFile, emails: &HashSet<String>) {
    match tokens.entries() {
        Ok(entries) => {
            for (email, _) in entries.iter().filter(|(e, _)| !emails.contains(e)) {
                delete_swept_token(tokens, email);
            }
        }
        Err(e) => tracing::warn!(error = %e, "couldn't read the stored tokens"),
    }
}

fn delete_swept_token(tokens: &TokenFile, email: &str) {
    tokens.delete(email).map_or_else(
        |e| tracing::warn!(error = %e, "couldn't delete a removed account's token"),
        |()| tracing::info!("deleted the token of an account removed before a quit"),
    );
}

struct Removal(watch::Sender<usize>);

impl Removal {
    fn start(removals: &watch::Sender<usize>) -> Self {
        removals.send_modify(|n| *n = n.saturating_add(1));
        Self(removals.clone())
    }
}

impl Drop for Removal {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n = n.saturating_sub(1));
    }
}

fn mark_signed_out(live: &Live, account: AccountId, grant: Grant) {
    let grants = Arc::clone(&live.grants);
    live.writer.queue(
        Priority::Sync,
        "mark an account signed out",
        move |tx, _| {
            if grants.current(account) != grant {
                tracing::debug!(
                    account = account.0,
                    "an older grant was refused; the account signed in since"
                );
                return Ok(());
            }
            if tx.read().accounts().get(account)?.is_some() {
                tx.accounts().set_needs_signin(account, true)?;
            }
            Ok(())
        },
    );
}

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
        mark_signed_out(&live, account, worker.grant());
    }
}
