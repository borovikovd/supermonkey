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
    pub fn from_grant(config: OAuthConfig, http: HttpClient, grant: Grant) -> Self {
        let tokens = Self::new(config, http, grant.refresh_token);
        *tokens.cached.lock() = Some(grant.access_token);
        tokens
    }

    /// The account's refresh token, to store or revoke.
    pub fn refresh_token(&self) -> &RefreshToken {
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
        self.cached.lock().clone().filter(AccessToken::is_fresh)
    }

    /// Fetches a new token and caches it. Call with `refreshing` held.
    async fn refresh(&self) -> Result<AccessToken, GmailError> {
        let token = endpoints::refresh(&self.config, &self.http, &self.refresh_token).await?;
        *self.cached.lock() = Some(token.clone());
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ub_fake_gmail::FakeGmail;

    use super::*;

    fn config(fake: &FakeGmail) -> OAuthConfig {
        OAuthConfig {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            auth_url: fake.auth_url(),
            token_url: fake.token_url(),
            revoke_url: fake.revoke_url(),
        }
    }

    #[tokio::test]
    async fn caches_the_token_and_refreshes_it_before_expiry() {
        let fake = FakeGmail::start();
        let user = fake.add_user("me@example.com");
        let tokens = Tokens::new(
            config(&fake),
            HttpClient::new().unwrap(),
            RefreshToken::new(user.refresh_token()),
        );

        let a = tokens.access_token().await.unwrap();
        let b = tokens.access_token().await.unwrap();
        assert_eq!(a.secret(), b.secret());
        assert_eq!(fake.count("POST", "/token"), 1);

        // Tokens that expire within the refresh margin are replaced on every use.
        fake.set_token_lifetime(Duration::from_secs(30));
        let c = tokens.refresh_after_401(&b).await.unwrap();
        let d = tokens.access_token().await.unwrap();
        assert_ne!(c.secret(), d.secret());
        assert_eq!(fake.count("POST", "/token"), 3);
    }

    #[tokio::test]
    async fn a_401_on_a_token_already_replaced_uses_the_new_one() {
        let fake = FakeGmail::start();
        let user = fake.add_user("me@example.com");
        let tokens = Tokens::new(
            config(&fake),
            HttpClient::new().unwrap(),
            RefreshToken::new(user.refresh_token()),
        );
        let old = tokens.access_token().await.unwrap();
        let new = tokens.refresh_after_401(&old).await.unwrap();
        // A second caller whose request also carried `old` gets `new`, not a third token.
        let again = tokens.refresh_after_401(&old).await.unwrap();
        assert_eq!(again.secret(), new.secret());
        assert_eq!(fake.count("POST", "/token"), 2);
    }
}
