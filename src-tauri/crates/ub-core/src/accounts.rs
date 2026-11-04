//! The `Accounts` handle (ARCHITECTURE.md, section 8.1): adding accounts,
//! signing in again, and waking the account workers.
//!
//! Extension points: the accounts and sync slice fills in [`Accounts::add`]
//! and [`Accounts::sign_in_again`] (the browser sign-in through
//! [`crate::Platform::open_sign_in`], then [`Accounts::register`]), starts an
//! account's worker from [`Accounts::register`] once the core has launched,
//! and makes [`Accounts::poll_now`] and [`Accounts::retry_now`] kick the
//! workers. Removing an account is the `removeAccount` action.

use std::sync::Arc;

use ub_domain::AccountId;
use ub_gmail::TokenStore;
use ub_store::{Priority, StoreError};

use crate::writer::CoreWriter;
use crate::{Error, GmailConfig};

/// Signing accounts in, and waking their workers.
#[derive(Clone)]
pub struct Accounts {
    writer: CoreWriter,
    #[expect(
        dead_code,
        reason = "the accounts slice reads and stores refresh tokens with it"
    )]
    tokens: Arc<dyn TokenStore>,
    #[expect(
        dead_code,
        reason = "the accounts slice signs in and builds each account's client with it"
    )]
    gmail: GmailConfig,
}

impl Accounts {
    pub(crate) fn new(writer: CoreWriter, tokens: Arc<dyn TokenStore>, gmail: GmailConfig) -> Self {
        Self {
            writer,
            tokens,
            gmail,
        }
    }

    /// Signs a new account in with Google in the browser and adds it (SPEC
    /// Accounts). A newer sign-in cancels one still waiting in the browser,
    /// which then fails with "Sign-in cancelled".
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "an extension point: the accounts slice runs the sign-in here"
    )]
    pub async fn add(&self) -> Result<AccountId, Error> {
        Err(Error::NotYetImplemented("adding an account"))
    }

    /// Signs an account in again, to renew a revoked grant or add a scope.
    #[expect(
        clippy::unused_async,
        clippy::unused_async_trait_impl,
        reason = "an extension point: the accounts slice runs the sign-in here"
    )]
    pub async fn sign_in_again(&self, account: AccountId) -> Result<(), Error> {
        tracing::debug!(account = account.0, "sign in again asked for");
        Err(Error::NotYetImplemented("signing in again"))
    }

    /// Adds the account for `email`, whose refresh token is already in the
    /// token store: the last step of a sign-in, and how the E2E server seeds
    /// a signed-in account. An account the app already has is marked signed
    /// in again. Returns its id.
    pub async fn register(&self, email: &str) -> Result<AccountId, Error> {
        let email = email.to_owned();
        let id = self
            .writer
            .call(Priority::Ui, move |tx, _| {
                if let Some(id) = tx.read().accounts().by_email(&email)? {
                    tx.accounts().set_needs_signin(id, false)?;
                    return Ok::<_, StoreError>(id);
                }
                tx.accounts().create(&email, &email)
            })
            .await?;
        Ok(id)
    }

    /// Polls every account's Gmail history now: mail arrived, or a test
    /// changed Gmail.
    pub fn poll_now(&self) {
        tracing::debug!("poll asked for; no account workers yet");
    }

    /// Retries every account's queued Gmail work now, without waiting out
    /// its back-off: the network came back.
    pub fn retry_now(&self) {
        tracing::debug!("retry asked for; no account workers yet");
    }
}
