//! The Gmail API client: OAuth, quota scheduling, typed outcomes
//! (ARCHITECTURE section 7).
//!
//! - [`Gmail`]: one REST client per account; every method takes a
//!   [`Priority`] and waits in the account's [`AccountScheduler`].
//! - [`Scheduler`]: per-account quota buckets and request slots.
//! - [`Outcome`] and [`GmailError`]: SPEC's Gmail errors table.
//! - [`retry`]: back-off with jitter and `Retry-After`.
//! - [`SignIn`], [`Tokens`], [`revoke`]: Google OAuth for an installed app.
//! - [`TokenStore`]: where refresh tokens live, a 0600 file or memory.
//!
//! ## Time
//!
//! The scheduler's bucket and the access tokens' expiry run on tokio's
//! monotonic clock. Both measure real-world time: Google's quota refills and
//! its tokens expire whatever the app's clock says, so the app's manual E2E
//! clock must not drive them (an E2E run at the production budget waits for
//! real refills). Tests control this time with tokio's paused clock
//! (`#[tokio::test(start_paused = true)]`, `tokio::time::advance`), from any
//! crate. Back-off waits, which are app behavior, take the caller's `sleep`
//! (see [`with_retry`]); the core sleeps those on its own clock.
//!
//! ## Wiring one account
//!
//! ```no_run
//! # async fn demo(store: &dyn ub_gmail::TokenStore) -> Result<(), Box<dyn std::error::Error>> {
//! use std::sync::Arc;
//! use ub_gmail::*;
//!
//! let http = HttpClient::new()?;
//! let config = OAuthConfig::google("client-id", "client-secret");
//! let scheduler = Scheduler::new(QuotaConfig::default()); // one per app
//!
//! // First sign-in: open the consent page in the system browser.
//! let sign_in = SignIn::start(&config).await?;
//! let _open_in_browser = sign_in.consent_url();
//! let grant = sign_in.finish(&http).await?;
//! let tokens = Arc::new(Tokens::from_grant(config.clone(), http.clone(), grant));
//! let probe = Gmail::new(GMAIL_BASE_URL, http.clone(), Arc::clone(&tokens), scheduler.account("sign-in"))?;
//! let email = probe.get_profile(Priority::P1).await?.email_address;
//! store.store(&email, tokens.refresh_token())?;
//!
//! // Later launches: from the stored refresh token.
//! if let Some(refresh_token) = store.load(&email)? {
//!     let tokens = Arc::new(Tokens::new(config, http.clone(), refresh_token));
//!     let gmail = Gmail::new(GMAIL_BASE_URL, http, tokens, scheduler.account(&email))?;
//!     let labels = gmail.list_labels(Priority::P3).await?;
//!     # let _ = labels;
//! }
//! # Ok(()) }
//! ```

mod client;
mod http;
mod ids;
mod oauth;
mod outcome;
mod quota;
pub mod retry;
mod token_store;
mod unsubscribe;
mod wire;

pub use client::{GMAIL_BASE_URL, Gmail, InvalidBaseUrl};
pub use http::{HttpClient, HttpClientError};
pub use ids::{FilterId, GmailAttachmentId, HistoryId, PageToken};
pub use oauth::{
    AccessToken, GMAIL_MODIFY_SCOPE, GMAIL_SETTINGS_SCOPE, Grant, OAuthConfig, RefreshToken,
    SignIn, SignInError, Tokens, revoke,
};
pub use outcome::{GmailError, Outcome};
pub use quota::{
    AccountScheduler, InvalidPriority, Permit, Priority, QuotaConfig, Scheduler, Units,
};
pub use retry::{Backoff, backoff, with_retry};
pub use token_store::{MemoryTokenStore, TokenFile, TokenStore, TokenStoreError};
pub use unsubscribe::one_click_unsubscribe;
pub use wire::{
    Draft, DraftPage, Filter, FilterAction, FilterCriteria, Format, HistoryLabelChange,
    HistoryMessage, HistoryPage, HistoryRecord, Label, LabelType, Message, Profile, SendAs, Thread,
    ThreadPage, ThreadQuery,
};
