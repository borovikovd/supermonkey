//! The `Accounts` handle (ARCHITECTURE.md, section 8.1): Google sign-in for a
//! new account or again for one that signed out, and waking the account
//! workers. Removing an account is the `removeAccount` action.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use ub_domain::AccountId;
use ub_gmail::{Gmail, Priority, RefreshToken, Scheduler, SignIn, Tokens};
use ub_store::{Priority as WritePriority, StoreError};

use crate::workers::{AccountWorkers, Grant};
use crate::writer::CoreWriter;
use crate::{Error, Platform};

/// Signing accounts in, and waking their workers.
#[derive(Clone)]
pub struct Accounts {
    writer: CoreWriter,
    workers: Arc<AccountWorkers>,
    platform: Arc<dyn Platform>,
    /// The sign-in waiting in the browser, if any, by its number.
    waiting: Arc<Mutex<Option<(u64, CancellationToken)>>>,
    next: Arc<AtomicU64>,
}

/// A sign-in that finished: whose grant it is.
struct SignedIn {
    email: String,
    tokens: Arc<Tokens>,
}

impl Accounts {
    pub(crate) fn new(
        writer: CoreWriter,
        workers: Arc<AccountWorkers>,
        platform: Arc<dyn Platform>,
    ) -> Self {
        Self {
            writer,
            workers,
            platform,
            waiting: Arc::default(),
            next: Arc::default(),
        }
    }

    /// Signs a new account in with Google in the browser and adds it (SPEC
    /// Accounts). An account the app already has is signed in again. A
    /// newer sign-in cancels one still waiting in the browser, which then
    /// fails with "Sign-in cancelled".
    pub async fn add(&self) -> Result<AccountId, Error> {
        let SignedIn { email, tokens } = self.sign_in().await?;
        self.keep_token(&email, tokens.refresh_token()).await?;
        let (id, grant) = self.signed_in_row(&email).await?;
        self.workers.signed_in(id, &email, tokens, grant)?;
        self.platform.focus_window();
        tracing::info!(account = id.0, "account added");
        Ok(id)
    }

    /// Signs an account in again, to renew a refused grant or add a scope.
    /// It must be the same Google account.
    pub async fn sign_in_again(&self, account: AccountId) -> Result<(), Error> {
        let expected = self
            .writer
            .call(WritePriority::Ui, move |tx, _| {
                Ok::<_, StoreError>(tx.read().accounts().get(account)?.map(|a| a.email))
            })
            .await?
            .ok_or_else(|| Error::Invalid("The account is gone".to_owned()))?;
        let SignedIn { email, tokens } = self.sign_in().await?;
        if !email.eq_ignore_ascii_case(&expected) {
            return Err(Error::Invalid(format!(
                "You signed in as {email}. Sign in as {expected} to reconnect this account."
            )));
        }
        self.keep_token(&expected, tokens.refresh_token()).await?;
        let (_, grant) = self.signed_in_row(&expected).await?;
        self.workers.signed_in(account, &expected, tokens, grant)?;
        tracing::info!(account = account.0, "signed in again");
        Ok(())
    }

    /// Adds the account for `email`, whose refresh token is already in the
    /// token store: how the E2E server seeds a signed-in account. An account
    /// the app already has is marked signed in again. Its worker starts once
    /// the core has launched. Returns its id.
    pub async fn register(&self, email: &str) -> Result<AccountId, Error> {
        let (id, grant) = self.signed_in_row(email).await?;
        self.workers.registered(id, email, grant).await?;
        Ok(id)
    }

    /// Polls every account's Gmail history now: mail arrived, or a test
    /// changed Gmail.
    pub fn poll_now(&self) {
        self.workers.poll_all();
    }

    /// Retries every account's Gmail work now, without waiting out its
    /// back-off: the network came back.
    pub fn retry_now(&self) {
        self.workers.retry_now();
    }

    /// The account row for a finished sign-in: a new one, or the existing
    /// one, no longer signed out. Its grant is renewed in the same job, so a
    /// sign-out reported for the old grant, whenever its job runs, changes
    /// nothing.
    async fn signed_in_row(&self, email: &str) -> Result<(AccountId, Grant), Error> {
        let (email, grants) = (email.to_owned(), Arc::clone(self.workers.grants()));
        let signed_in = self
            .writer
            .call(WritePriority::Ui, move |tx, _| {
                let id = match tx.read().accounts().by_email(&email)? {
                    Some(id) => {
                        tx.accounts().set_needs_signin(id, false)?;
                        id
                    }
                    None => tx.accounts().create(&email, &email)?,
                };
                Ok::<_, StoreError>((id, grants.renew(id)))
            })
            .await?;
        Ok(signed_in)
    }

    /// Stores the account's refresh token for the next launch, once no
    /// removed account is still being forgotten: its clean-up deletes the
    /// token of its address.
    async fn keep_token(&self, email: &str, token: &RefreshToken) -> Result<(), Error> {
        self.workers.removals_done().await;
        let (store, email, token) = (
            Arc::clone(self.workers.tokens()),
            email.to_owned(),
            token.clone(),
        );
        tokio::task::spawn_blocking(move || store.store(&email, &token)).await??;
        Ok(())
    }

    /// Runs a Google sign-in in the system browser, after any removal still
    /// revoking (its revoke would end the new grant too). It replaces one
    /// still waiting in the browser.
    async fn sign_in(&self) -> Result<SignedIn, Error> {
        self.workers.removals_done().await;
        let number = self.next.fetch_add(1, Ordering::SeqCst);
        let cancel = CancellationToken::new();
        let previous = self.waiting.lock().replace((number, cancel.clone()));
        if let Some((_, previous)) = previous {
            previous.cancel();
        }
        let result = tokio::select! {
            () = cancel.cancelled() => Err(Error::SignInCancelled),
            signed_in = self.sign_in_once() => signed_in,
        };
        let mut waiting = self.waiting.lock();
        if waiting.as_ref().is_some_and(|(n, _)| *n == number) {
            *waiting = None;
        }
        drop(waiting);
        result
    }

    async fn sign_in_once(&self) -> Result<SignedIn, Error> {
        let config = self.workers.gmail_config();
        if config.oauth.client_id.is_empty() {
            return Err(Error::Invalid(
                "This build has no Google OAuth client. Put google-oauth-client.json in the project and rebuild."
                    .to_owned(),
            ));
        }
        let http = self.workers.http();
        let sign_in = SignIn::start(&config.oauth).await?;
        self.platform.open_sign_in(sign_in.consent_url())?;
        let grant = sign_in.finish(http).await?;
        let tokens = Arc::new(Tokens::from_grant(
            config.oauth.clone(),
            http.clone(),
            grant,
        ));
        // Its own scheduler: one request, for an address not known yet, and
        // nothing left behind in the accounts' schedulers.
        let probe = Gmail::new(
            &config.base_url,
            http.clone(),
            Arc::clone(&tokens),
            Scheduler::new(config.quota).account("sign-in"),
        )
        .map_err(|e| Error::Invalid(e.to_string()))?;
        let profile = probe.get_profile(Priority::P1).await?;
        Ok(SignedIn {
            email: profile.email_address,
            tokens,
        })
    }
}
