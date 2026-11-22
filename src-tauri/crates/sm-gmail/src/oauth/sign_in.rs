use std::fmt;

use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, ClientId, CsrfToken, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
};

use super::endpoints;
use super::loopback::{Loopback, Redirect};
use super::{
    AccessToken, GMAIL_MODIFY_SCOPE, GMAIL_SETTINGS_SCOPE, Grant, OAuthConfig, RefreshToken,
};
use crate::http::HttpClient;
use crate::outcome::GmailError;

#[derive(Debug, thiserror::Error)]
pub enum SignInError {
    #[error("could not open the sign-in port: {0}")]
    Loopback(#[source] std::io::Error),
    #[error("the OAuth consent URL is not valid")]
    InvalidConfig,
    #[error("Google did not authorize the app: {0}")]
    Denied(String),
    #[error("Google returned no refresh token")]
    NoRefreshToken,
    #[error("could not exchange the sign-in code: {0}")]
    Exchange(#[from] GmailError),
}

pub struct SignIn {
    config: OAuthConfig,
    loopback: Loopback,
    redirect_uri: String,
    consent_url: String,
    state: CsrfToken,
    verifier: PkceCodeVerifier,
}

impl SignIn {
    pub async fn start(config: OAuthConfig) -> Result<Self, SignInError> {
        let loopback = Loopback::bind().await.map_err(SignInError::Loopback)?;
        let redirect_uri = loopback.redirect_uri();
        let auth_url =
            AuthUrl::new(config.auth_url.clone()).map_err(|_bad| SignInError::InvalidConfig)?;
        let redirect =
            RedirectUrl::new(redirect_uri.clone()).map_err(|_bad| SignInError::InvalidConfig)?;
        let client = BasicClient::new(ClientId::new(config.client_id.clone()))
            .set_auth_uri(auth_url)
            .set_redirect_uri(redirect);
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state) = client
            .authorize_url(CsrfToken::new_random)
            .add_scope(Scope::new(GMAIL_MODIFY_SCOPE.to_owned()))
            .add_scope(Scope::new(GMAIL_SETTINGS_SCOPE.to_owned()))
            .set_pkce_challenge(challenge)
            .add_extra_param("access_type", "offline")
            .add_extra_param("prompt", "consent")
            .url();
        Ok(Self {
            config,
            loopback,
            redirect_uri,
            consent_url: url.into(),
            state,
            verifier,
        })
    }

    #[must_use]
    pub fn consent_url(&self) -> &str {
        &self.consent_url
    }

    pub async fn finish(
        self,
        http: &HttpClient,
        ready: impl Future<Output = ()>,
    ) -> Result<Grant, SignInError> {
        let code = match self.loopback.wait(self.state.secret()).await {
            Redirect::Code(code) => code,
            Redirect::Error(error) => return Err(SignInError::Denied(error)),
        };
        ready.await;
        let form = [
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("client_id", self.config.client_id.as_str()),
            ("client_secret", self.config.client_secret.as_str()),
            ("code_verifier", self.verifier.secret().as_str()),
        ];
        let answer = endpoints::token_request(&self.config, http, &form).await?;
        let refresh_token = answer.refresh_token.ok_or(SignInError::NoRefreshToken)?;
        tracing::info!("sign-in finished");
        Ok(Grant {
            refresh_token: RefreshToken::new(refresh_token),
            access_token: AccessToken::issued(answer.access_token, answer.expires_in),
        })
    }
}

impl fmt::Debug for SignIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignIn")
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}
