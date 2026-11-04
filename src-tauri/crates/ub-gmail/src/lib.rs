//! Gmail for Ubermensch: the REST client, error outcomes, quota scheduling, Google OAuth
//! and refresh-token storage.
//!
//! Wiring for one account:
//!
//! ```no_run
//! # async fn demo(store: &dyn ub_gmail::TokenStore) -> Result<(), ub_gmail::GmailError> {
//! use std::sync::Arc;
//! use ub_gmail::*;
//!
//! let http = default_http_client();
//! let oauth = OAuthConfig::google("client-id", "client-secret");
//! let refresh_token = store.load_refresh_token("me@example.com").unwrap().unwrap();
//! let tokens = Arc::new(OAuthTokens::new(oauth, http.clone(), refresh_token));
//! let gmail = Gmail::new(GMAIL_BASE_URL, http, tokens);
//!
//! let scheduler = Scheduler::new(QuotaConfig::default()); // one per app
//! let account = scheduler.account("me@example.com");
//! let thread = account
//!     .run(Priority::P2, cost::threads_get(Format::Full), gmail.get_thread("t1", Format::Full, &[]))
//!     .await?;
//! # let _ = thread; Ok(()) }
//! ```

mod client;
mod error;
mod oauth;
mod quota;
pub mod retry;
mod tokens;
mod types;

pub use client::{GMAIL_BASE_URL, Gmail, ThreadListQuery, default_http_client};
pub use error::{GmailError, Outcome};
pub use oauth::{
    AccessToken, AuthResult, GMAIL_SCOPE, OAuthConfig, OAuthTokens, RefreshToken, authorize,
    refresh, revoke,
};
pub use quota::{AccountScheduler, Permit, Priority, QuotaBucket, QuotaConfig, Scheduler, cost};
pub use retry::{backoff, with_retry};
pub use tokens::{MemoryTokenStore, TokenFile, TokenStore, TokenStoreError};
pub use types::*;
