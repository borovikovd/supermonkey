use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use sm_domain::AccountId;
use sm_gmail::{Gmail, Priority, RefreshToken, Scheduler, SignIn, Tokens};
use sm_store::{Priority as WritePriority, StoreError};
use tokio_util::sync::CancellationToken;

use crate::workers::{AccountWorkers, Grant};
use crate::writer::CoreWriter;
use crate::{Error, Platform};

pub struct Accounts {
    writer: CoreWriter,
    workers: Arc<AccountWorkers>,
    platform: Arc<dyn Platform>,
    waiting: Mutex<Option<(u64, CancellationToken)>>,
    next: AtomicU64,
}

impl fmt::Debug for Accounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Accounts").finish_non_exhaustive()
    }
}

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
            waiting: Mutex::default(),
            next: AtomicU64::default(),
        }
    }

    pub async fn add(&self) -> Result<AccountId, Error> {
        let SignedIn { email, tokens } = self.sign_in().await?;
        let id = self.start_account(&email, tokens).await?;
        self.platform.focus_window();
        tracing::info!(account = id.0, "account added");
        Ok(id)
    }

    pub async fn sign_in_again(&self, account: AccountId) -> Result<(), Error> {
        let SignedIn { email, tokens } = self.sign_in_as(account).await?;
        let id = self.start_account(&email, tokens).await?;
        tracing::info!(account = id.0, "signed in again");
        Ok(())
    }

    async fn sign_in_as(&self, account: AccountId) -> Result<SignedIn, Error> {
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
        Ok(SignedIn {
            email: expected,
            tokens,
        })
    }

    async fn start_account(&self, email: &str, tokens: Arc<Tokens>) -> Result<AccountId, Error> {
        self.keep_token(email, tokens.refresh_token()).await?;
        let (id, grant) = self.signed_in_row(email).await?;
        self.workers.signed_in(id, email, tokens, grant)?;
        Ok(id)
    }

    pub async fn register(&self, email: &str) -> Result<AccountId, Error> {
        let (id, grant) = self.signed_in_row(email).await?;
        self.workers.registered(id, email, grant).await?;
        Ok(id)
    }

    pub fn poll_now(&self) {
        self.workers.poll_all();
    }

    pub fn retry_now(&self) {
        self.workers.retry_now();
    }

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

    async fn keep_token(&self, email: &str, token: &RefreshToken) -> Result<(), Error> {
        self.workers.removals_done().await;
        let (store, email, token) = (
            Arc::clone(self.workers.tokens()),
            email.to_owned(),
            token.clone(),
        );
        tokio::task::spawn_blocking(move || store.store(email, token)).await??;
        Ok(())
    }

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
        let sign_in = SignIn::start(config.oauth.clone()).await?;
        self.platform.open_sign_in(sign_in.consent_url())?;
        let grant = sign_in.finish(http, self.workers.removals_done()).await?;
        let tokens = Arc::new(Tokens::from_grant(
            config.oauth.clone(),
            http.clone(),
            grant,
        ));
        let probe = Gmail::new(
            &config.base_url,
            http.clone(),
            Arc::clone(&tokens),
            Scheduler::new(config.quota).account("sign-in"),
        )?;
        let profile = probe.get_profile(Priority::P1).await?;
        Ok(SignedIn {
            email: profile.email_address,
            tokens,
        })
    }
}
