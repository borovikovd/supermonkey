use parking_lot::Mutex;

use super::endpoints;
use super::{AccessToken, Grant, OAuthConfig, RefreshToken};
use crate::http::HttpClient;
use crate::outcome::GmailError;

#[derive(Debug)]
pub struct Tokens {
    config: OAuthConfig,
    http: HttpClient,
    refresh_token: RefreshToken,
    cached: Mutex<Option<AccessToken>>,
    #[expect(
        clippy::disallowed_types,
        reason = "held across the refresh request's `.await`, so a second caller waits for that token"
    )]
    refreshing: tokio::sync::Mutex<()>,
}

impl Tokens {
    #[must_use]
    pub fn new(config: OAuthConfig, http: HttpClient, refresh_token: RefreshToken) -> Self {
        Self {
            config,
            http,
            refresh_token,
            cached: Mutex::new(None),
            #[expect(
                clippy::disallowed_types,
                reason = "held across the refresh request's `.await`, so a second caller waits for that token"
            )]
            refreshing: tokio::sync::Mutex::new(()),
        }
    }

    #[must_use]
    pub fn from_grant(config: OAuthConfig, http: HttpClient, grant: Grant) -> Self {
        let tokens = Self::new(config, http, grant.refresh_token);
        *tokens.cached.lock() = Some(grant.access_token);
        tokens
    }

    pub const fn refresh_token(&self) -> &RefreshToken {
        &self.refresh_token
    }

    pub(crate) async fn access_token(&self) -> Result<AccessToken, GmailError> {
        if let Some(token) = self.fresh() {
            return Ok(token);
        }
        let _refreshing = self.refreshing.lock().await;
        if let Some(token) = self.fresh() {
            return Ok(token);
        }
        self.refresh().await
    }

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
        let cached = self.cached.lock().clone();
        cached.filter(AccessToken::is_fresh)
    }

    async fn refresh(&self) -> Result<AccessToken, GmailError> {
        let token = endpoints::refresh(&self.config, &self.http, &self.refresh_token).await?;
        *self.cached.lock() = Some(token.clone());
        Ok(token)
    }
}
