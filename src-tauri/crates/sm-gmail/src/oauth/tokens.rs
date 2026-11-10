//! One account's access tokens: cached, refreshed early, refreshed once for
//! any number of concurrent 401s.

use parking_lot::Mutex;

use super::endpoints;
use super::{AccessToken, Grant, OAuthConfig, RefreshToken};
use crate::http::HttpClient;
use crate::outcome::GmailError;

/// Hands an account's Gmail client its bearer tokens. The access token is
/// cached in memory and refreshed with the refresh token shortly before it
/// expires, or after a 401. Concurrent callers share one refresh.
#[derive(Debug)]
pub struct Tokens {
    config: OAuthConfig,
    http: HttpClient,
    refresh_token: RefreshToken,
    cached: Mutex<Option<AccessToken>>,
    /// Held across a refresh request, so callers that need a new token while
    /// one is being fetched wait for it instead of fetching their own. An
    /// async mutex because it is held across that `.await`.
    refreshing: tokio::sync::Mutex<()>,
}

impl Tokens {
    /// Tokens for a stored refresh token, with no access token yet.
    #[must_use]
    pub fn new(config: OAuthConfig, http: HttpClient, refresh_token: RefreshToken) -> Self {
        Self {
            config,
            http,
            refresh_token,
            cached: Mutex::new(None),
            refreshing: tokio::sync::Mutex::new(()),
        }
    }

    /// Tokens for a sign-in that just finished, seeded with its access token.
    #[must_use]
    pub fn from_grant(config: OAuthConfig, http: HttpClient, grant: Grant) -> Self {
        let tokens = Self::new(config, http, grant.refresh_token);
        *tokens.cached.lock() = Some(grant.access_token);
        tokens
    }

    /// The account's refresh token, to store or revoke.
    pub const fn refresh_token(&self) -> &RefreshToken {
        &self.refresh_token
    }

    /// An access token good for at least the refresh margin.
    pub(crate) async fn access_token(&self) -> Result<AccessToken, GmailError> {
        if let Some(token) = self.fresh() {
            return Ok(token);
        }
        let _refreshing = self.refreshing.lock().await;
        // Someone else may have refreshed while this caller waited.
        if let Some(token) = self.fresh() {
            return Ok(token);
        }
        self.refresh().await
    }

    /// A token to retry with after Gmail answered 401 to `rejected`: the one
    /// another caller already fetched, or a new one.
    pub(crate) async fn refresh_after_401(
        &self,
        rejected: &AccessToken,
    ) -> Result<AccessToken, GmailError> {
        let _refreshing = self.refreshing.lock().await;
        if let Some(token) = self.fresh().filter(|t| t.secret() != rejected.secret()) {
            return Ok(token);
        }
        self.refresh().await
    }

    fn fresh(&self) -> Option<AccessToken> {
        // Copied out first: the freshness check reads the clock, which runs
        // with no lock held.
        let cached = self.cached.lock().clone();
        cached.filter(AccessToken::is_fresh)
    }

    /// Fetches a new token and caches it. Call with `refreshing` held.
    async fn refresh(&self) -> Result<AccessToken, GmailError> {
        let token = endpoints::refresh(&self.config, &self.http, &self.refresh_token).await?;
        *self.cached.lock() = Some(token.clone());
        Ok(token)
    }
}
