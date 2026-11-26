pub(crate) mod drafts;
pub(crate) mod ingest;
pub(crate) mod ops;
pub(crate) mod outbox;
pub(crate) mod queue;
pub(crate) mod worker;

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::domain::{AccountId, OpQueue, SyncStatus, ThreadId};
use crate::gmail::{
    Format, Gmail, GmailError, HttpClient, OAuthConfig, Outcome, Priority as GmailPriority,
    Scheduler, SignIn, SignInError, TokenFile, Tokens, revoke,
};
use crate::store::{Priority as StorePriority, ReadPool, StoreError, Writer};

use super::drafts::DraftFiles;
use super::events::Effect;
use super::network::Network;
use super::runtime::{Activity, Clock, Tasks};
use super::writer_state::{SubOp, WriterState};
use super::{CoreError, Platform};
use ops::run_op_worker;
use outbox::run_outbox;
use queue::FetchPriority;
use worker::{AccountSlot, run_sync_loop, spawn_fetch_workers};

pub(crate) struct AccountGmail {
    slot: Arc<AccountSlot>,
    grant_gen: u64,
    gmail: Arc<Gmail>,
}

impl std::fmt::Debug for AccountGmail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountGmail")
            .field("account", &self.slot.account)
            .finish_non_exhaustive()
    }
}

impl AccountGmail {
    #[must_use]
    pub(crate) fn gmail(&self) -> &Gmail {
        &self.gmail
    }

    pub(crate) fn ensure_online(&self, msg: &'static str) -> Result<(), CoreError> {
        if self.slot.is_online() {
            Ok(())
        } else {
            Err(CoreError::other(msg))
        }
    }

    pub(crate) async fn check<T>(&self, res: Result<T, GmailError>) -> Result<T, GmailError> {
        if let Err(err) = &res {
            if matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized) {
                self.slot.mark_auth_failed(self.grant_gen).await;
            }
            self.slot.network.lost_if_unreachable(err).await;
        }
        res
    }
}

struct ManagedAccount {
    slot: Arc<AccountSlot>,
    tasks: Tasks,
}

#[derive(Clone)]
pub(crate) struct AccountManager {
    inner: Arc<ManagerInner>,
}

struct ManagerInner {
    http: HttpClient,
    gmail_base_url: String,
    oauth: OAuthConfig,
    tokens: Arc<TokenFile>,
    scheduler: Scheduler,
    pool: Arc<ReadPool>,
    writer: Writer<WriterState>,
    clock: Clock,
    network: Network,
    activity: Activity,
    files: DraftFiles,
    root_tasks: Tasks,
    accounts: Mutex<HashMap<AccountId, ManagedAccount>>,
    active_sign_in: Mutex<Option<(u64, CancellationToken)>>,
    next_sign_in_id: std::sync::atomic::AtomicU64,
    revoking: watch::Sender<usize>,
}

struct RevokeGuard {
    tx: watch::Sender<usize>,
}

impl RevokeGuard {
    fn new(tx: watch::Sender<usize>) -> Self {
        tx.send_modify(|n| *n = n.saturating_add(1));
        Self { tx }
    }
}

impl Drop for RevokeGuard {
    fn drop(&mut self) {
        self.tx.send_modify(|n| *n = n.saturating_sub(1));
    }
}

impl std::fmt::Debug for AccountManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountManager").finish_non_exhaustive()
    }
}

impl AccountManager {
    #[expect(
        clippy::too_many_arguments,
        reason = "account manager wires Gmail client config, storage, scheduler, and task supervision"
    )]
    #[must_use]
    pub(crate) fn new(
        http: HttpClient,
        gmail_base_url: String,
        oauth: OAuthConfig,
        tokens: Arc<TokenFile>,
        scheduler: Scheduler,
        pool: Arc<ReadPool>,
        writer: Writer<WriterState>,
        clock: Clock,
        network: Network,
        activity: Activity,
        files: DraftFiles,
        root_tasks: Tasks,
    ) -> Self {
        let (revoking, _revoking_rx) = watch::channel(0usize);
        Self {
            inner: Arc::new(ManagerInner {
                http,
                gmail_base_url,
                oauth,
                tokens,
                scheduler,
                pool,
                writer,
                clock,
                network,
                activity,
                files,
                root_tasks,
                accounts: Mutex::new(HashMap::new()),
                active_sign_in: Mutex::new(None),
                next_sign_in_id: std::sync::atomic::AtomicU64::new(1),
                revoking,
            }),
        }
    }

    pub(crate) async fn start_existing(&self) -> Result<(), StoreError> {
        #[expect(
            clippy::redundant_closure_for_method_calls,
            reason = "Db carries a lifetime so a closure is needed for ReadPool::with HRTB"
        )]
        let records = self.inner.pool.with(|db| db.accounts())?;
        for rec in records {
            let token = self.inner.tokens.load(&rec.email).ok().flatten();
            let gmail = token.and_then(|refresh| {
                let tok = Tokens::new(
                    self.inner.http.clone(),
                    self.inner.oauth.clone(),
                    refresh,
                    None,
                );
                let sched = self.inner.scheduler.account(&rec.email);
                Gmail::new(
                    self.inner.http.clone(),
                    &self.inner.gmail_base_url,
                    tok,
                    sched,
                )
                .ok()
                .map(Arc::new)
            });
            if gmail.is_none() && !rec.needs_signin {
                let id = rec.id;
                super::log_err(
                    "mark missing token account as needs_signin",
                    self.inner
                        .writer
                        .call(StorePriority::Sync, move |tx, _state| {
                            tx.set_needs_signin(id, true)?;
                            tx.set_sync_status(id, SyncStatus::NeedsSignin)?;
                            Ok::<_, StoreError>(())
                        })
                        .await,
                );
            }
            let slot = self.ensure_account_running(rec.id, rec.email, gmail);
            slot.kick_poll();
            slot.kick_ops();
            slot.kick_outbox();
        }
        Ok(())
    }

    #[must_use]
    pub(crate) fn slot(&self, account: AccountId) -> Option<Arc<AccountSlot>> {
        self.inner
            .accounts
            .lock()
            .get(&account)
            .map(|m| Arc::clone(&m.slot))
    }

    #[must_use]
    pub(crate) fn gmail_for(&self, account: AccountId) -> Option<(u64, Arc<Gmail>)> {
        self.slot(account).and_then(|s| s.gmail())
    }

    pub(crate) fn gmail_session(
        &self,
        account: AccountId,
        signin_msg: &'static str,
    ) -> Result<AccountGmail, CoreError> {
        let slot = self
            .slot(account)
            .ok_or_else(|| CoreError::other(signin_msg))?;
        let (grant_gen, gmail) = slot.gmail().ok_or_else(|| CoreError::other(signin_msg))?;
        Ok(AccountGmail {
            slot,
            grant_gen,
            gmail,
        })
    }

    #[must_use]
    fn all_slots(&self) -> Vec<Arc<AccountSlot>> {
        self.inner
            .accounts
            .lock()
            .values()
            .map(|m| Arc::clone(&m.slot))
            .collect()
    }

    pub(crate) fn kick_ops(&self, account: AccountId) {
        if let Some(slot) = self.slot(account) {
            slot.kick_ops();
        }
    }

    pub(crate) fn kick_outbox(&self, account: AccountId) {
        if let Some(slot) = self.slot(account) {
            slot.kick_outbox();
        }
    }

    pub(crate) fn poll_all(&self) {
        for slot in self.all_slots() {
            slot.kick_poll();
        }
    }

    pub(crate) fn on_focus(&self) {
        for slot in self.all_slots() {
            slot.kick_poll();
            if !slot.is_online() {
                continue;
            }
            let cancel = self.inner.root_tasks.token();
            self.inner.root_tasks.spawn_busy(async move {
                if cancel.is_cancelled() || !slot.is_online() {
                    return;
                }
                let Some((grant_gen, gmail)) = slot.gmail() else {
                    return;
                };
                let Some((r1, r2)) = cancel
                    .run_until_cancelled(async {
                        tokio::join!(
                            worker::refresh_labels_guarded(&slot, &gmail, GmailPriority::P2),
                            worker::refresh_send_as_guarded(&slot, &gmail, GmailPriority::P2),
                        )
                    })
                    .await
                else {
                    return;
                };
                let auth_err = |res: &Result<(), GmailError>| {
                    res.as_ref().is_err_and(|err| {
                        matches!(err.outcome, Outcome::InvalidGrant | Outcome::Unauthorized)
                    })
                };
                if auth_err(&r1) || auth_err(&r2) {
                    slot.mark_auth_failed(grant_gen).await;
                }
            });
        }
    }

    #[cfg(any(test, feature = "fake"))]
    pub(crate) fn wake_retrying(&self) {
        for slot in self.all_slots() {
            slot.kick_ops();
            slot.kick_outbox();
            if slot.has_failure() {
                slot.kick_poll();
            }
        }
    }

    pub(crate) fn on_online(&self) {
        for slot in self.all_slots() {
            slot.kick_poll();
            slot.kick_ops();
            slot.kick_outbox();
        }
    }

    pub(crate) fn on_offline(&self) {
        let slots = self.all_slots();
        self.inner.root_tasks.spawn_busy(async move {
            for slot in slots {
                if slot.gmail().is_some() {
                    slot.set_status(SyncStatus::Offline).await;
                }
            }
        });
    }

    pub(crate) fn open_conversation(&self, account: AccountId, thread: ThreadId) {
        let Some(slot) = self.slot(account) else {
            return;
        };
        if !slot.is_online() {
            return;
        }
        let needs = self
            .inner
            .pool
            .with(|db| {
                let Some((_, gtid)) = db.thread_gmail_id(thread)? else {
                    return Ok::<_, StoreError>(None);
                };
                let missing = !db.threads_needing_bodies(&[thread])?.is_empty();
                Ok(missing.then_some(gtid))
            })
            .ok()
            .flatten();
        if let Some(gtid) = needs {
            slot.fetch_queue.enqueue(
                gtid,
                Format::Full,
                FetchPriority::OpenConversation,
                false,
                None,
                &self.inner.activity,
            );
        }
    }

    pub(crate) async fn add_account(
        &self,
        platform: &dyn Platform,
    ) -> Result<AccountId, CoreError> {
        let (grant, email, display_name) = self.run_oauth(platform, None).await?;
        self.inner
            .tokens
            .store_async(email.clone(), grant.refresh_token.clone())
            .await
            .map_err(|e| CoreError::internal(e.to_string()))?;

        let email_for_db = email.clone();
        let name_for_db = display_name.clone();
        let account_id = self
            .inner
            .writer
            .call(StorePriority::Ui, move |tx, state| {
                let rec = match tx.read().account_by_email(&email_for_db)? {
                    Some(existing) => {
                        tx.set_needs_signin(existing.id, false)?;
                        tx.set_sync_status(existing.id, SyncStatus::Syncing)?;
                        tx.clear_history(existing.id)?;
                        state.stage_sub(SubOp::StopSearching(existing.id));
                        state.emit(Effect::CancelAccountListings(existing.id));
                        state.emit(Effect::FetchSubscribedBodies);
                        existing
                    }
                    None => tx.create_account(&email_for_db, &name_for_db)?,
                };
                Ok::<_, StoreError>(rec.id)
            })
            .await
            .map_err(|e| CoreError::internal(e.to_string()))?;

        let gmail = self.make_gmail(&email, grant)?;
        let slot = self.ensure_account_running(account_id, email, Some(gmail));
        slot.kick_poll();
        slot.kick_ops();
        slot.kick_outbox();
        Ok(account_id)
    }

    pub(crate) async fn sign_in_again(
        &self,
        account: AccountId,
        platform: &dyn Platform,
    ) -> Result<(), CoreError> {
        let existing = self
            .inner
            .pool
            .with(|db| db.account(account))
            .map_err(|e| CoreError::internal(e.to_string()))?
            .ok_or_else(|| CoreError::other("That account is no longer in Supermonkey"))?;

        let (grant, returned_email, display_name) =
            self.run_oauth(platform, Some(&existing.email)).await?;
        if !returned_email.eq_ignore_ascii_case(&existing.email) {
            return Err(CoreError::other(format!(
                "Signed in as {returned_email}, expected {}",
                existing.email
            )));
        }

        self.inner
            .tokens
            .store_async(existing.email.clone(), grant.refresh_token.clone())
            .await
            .map_err(|e| CoreError::internal(e.to_string()))?;

        let email_for_db = existing.email.clone();
        let target_id = self
            .inner
            .writer
            .call(StorePriority::Ui, move |tx, state| {
                let id = if tx.read().account(account)?.is_some() {
                    tx.set_needs_signin(account, false)?;
                    tx.set_sync_status(account, SyncStatus::Idle)?;
                    account
                } else if let Some(found) = tx.read().account_by_email(&email_for_db)? {
                    tx.set_needs_signin(found.id, false)?;
                    tx.set_sync_status(found.id, SyncStatus::Idle)?;
                    found.id
                } else {
                    tx.create_account(&email_for_db, &display_name)?.id
                };
                state.emit(Effect::FetchSubscribedBodies);
                Ok::<_, StoreError>(id)
            })
            .await
            .map_err(|e| CoreError::internal(e.to_string()))?;

        let gmail = self.make_gmail(&existing.email, grant)?;
        let slot = self.ensure_account_running(target_id, existing.email, Some(gmail));
        slot.kick_poll();
        slot.kick_ops();
        slot.kick_outbox();
        Ok(())
    }

    pub(crate) fn remove_account(&self, account: AccountId, email: String) {
        let managed = self.inner.accounts.lock().remove(&account);
        let revoke_guard = RevokeGuard::new(self.inner.revoking.clone());
        let inner = Arc::clone(&self.inner);
        let cancel = self.inner.root_tasks.token();
        self.inner.root_tasks.spawn_busy(async move {
            let _revoke_guard = revoke_guard;
            if let Some(m) = managed {
                m.slot.fetch_queue.clear();
                m.tasks.stop().await;
            }
            inner.scheduler.forget(&email);
            let old_token = inner.tokens.load_async(email.clone()).await.ok().flatten();
            super::log_err(
                "delete account token",
                inner.tokens.delete_async(email).await,
            );
            if let Some(tok) = old_token
                && let Some(res) = cancel
                    .run_until_cancelled(revoke(&inner.http, &inner.oauth, &tok))
                    .await
            {
                super::log_err("revoke account token", res);
            }
        });
    }

    fn make_gmail(&self, email: &str, grant: crate::gmail::Grant) -> Result<Arc<Gmail>, CoreError> {
        let tokens = Tokens::from_grant(self.inner.http.clone(), self.inner.oauth.clone(), grant);
        let sched = self.inner.scheduler.account(email);
        Gmail::new(
            self.inner.http.clone(),
            &self.inner.gmail_base_url,
            tokens,
            sched,
        )
        .map(Arc::new)
        .map_err(|e| CoreError::internal(e.to_string()))
    }

    fn ensure_account_running(
        &self,
        account: AccountId,
        email: String,
        gmail: Option<Arc<Gmail>>,
    ) -> Arc<AccountSlot> {
        let mut map = self.inner.accounts.lock();
        if let Some(existing) = map.get(&account) {
            if let Some(g) = gmail {
                existing.slot.set_gmail(g);
            }
            return Arc::clone(&existing.slot);
        }

        let tasks = self.inner.root_tasks.child();
        let slot = Arc::new(AccountSlot::new(
            account,
            email,
            gmail,
            self.inner.network.clone(),
            tasks.clone(),
            self.inner.writer.clone(),
            Arc::clone(&self.inner.pool),
            self.inner.clock.clone(),
            self.inner.activity.clone(),
            self.inner.files.clone(),
        ));

        spawn_fetch_workers(&slot, &tasks);

        let cancel = tasks.token();
        tasks.spawn(run_op_worker(
            Arc::clone(&slot),
            OpQueue::Mail,
            cancel.clone(),
        ));
        tasks.spawn(run_op_worker(
            Arc::clone(&slot),
            OpQueue::Drafts,
            cancel.clone(),
        ));
        tasks.spawn(run_outbox(Arc::clone(&slot), cancel.clone()));
        tasks.spawn(run_sync_loop(Arc::clone(&slot), tasks.clone(), cancel));

        map.insert(
            account,
            ManagedAccount {
                slot: Arc::clone(&slot),
                tasks,
            },
        );
        slot
    }

    async fn run_oauth(
        &self,
        platform: &dyn Platform,
        login_hint: Option<&str>,
    ) -> Result<(crate::gmail::Grant, String, String), CoreError> {
        let _sign_in_guard = self.inner.activity.enter_sign_in();
        let sign_in_id = self
            .inner
            .next_sign_in_id
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let my_cancel = self.inner.root_tasks.token().child_token();
        {
            let mut active = self.inner.active_sign_in.lock();
            if let Some((_, prev)) = active.replace((sign_in_id, my_cancel.clone())) {
                prev.cancel();
            }
        }
        let _cleanup = SignInSlotCleanup {
            inner: Arc::clone(&self.inner),
            id: sign_in_id,
        };

        let (flow, auth_url) = SignIn::start(&self.inner.oauth, login_hint)
            .await
            .map_err(|e| map_sign_in_error(&e))?;
        platform.open_url(&auth_url)?;

        let callback = tokio::select! {
            biased;
            () = my_cancel.cancelled() => return Err(CoreError::sign_in_cancelled()),
            res = flow.wait_redirect() => res.map_err(|e| map_sign_in_error(&e))?,
        };

        let mut rev_rx = self.inner.revoking.subscribe();
        tokio::select! {
            biased;
            () = my_cancel.cancelled() => return Err(CoreError::sign_in_cancelled()),
            _ = rev_rx.wait_for(|&n| n == 0) => {}
        }

        let grant = tokio::select! {
            biased;
            () = my_cancel.cancelled() => return Err(CoreError::sign_in_cancelled()),
            res = SignIn::exchange(&self.inner.http, &self.inner.oauth, callback) => {
                res.map_err(|e| map_sign_in_error(&e))?
            }
        };

        let probe_gmail = self.make_gmail("oauth-probe", grant.clone())?;
        let profile = tokio::select! {
            biased;
            () = my_cancel.cancelled() => return Err(CoreError::sign_in_cancelled()),
            res = probe_gmail.get_profile(GmailPriority::P1) => {
                res.map_err(|e| CoreError::other(e.to_string()))?
            }
        };
        let display_name = profile
            .email_address
            .split('@')
            .next()
            .unwrap_or(&profile.email_address)
            .to_owned();
        Ok((grant, profile.email_address, display_name))
    }
}

struct SignInSlotCleanup {
    inner: Arc<ManagerInner>,
    id: u64,
}

impl Drop for SignInSlotCleanup {
    fn drop(&mut self) {
        let mut active = self.inner.active_sign_in.lock();
        if active.as_ref().is_some_and(|(cur, _)| *cur == self.id) {
            *active = None;
        }
    }
}

fn map_sign_in_error(err: &SignInError) -> CoreError {
    match err {
        SignInError::Denied(code) => CoreError::other(format!("Sign-in failed: {code}")),
        SignInError::Token(g) => CoreError::other(format!("Sign-in failed: {g}")),
        SignInError::NotConfigured
        | SignInError::InvalidUrl(_)
        | SignInError::Bind(_)
        | SignInError::NoRefreshToken => CoreError::other(format!("Sign-in failed: {err}")),
    }
}
