//! The installed-app sign-in: consent in the system browser, back to a
//! loopback port, PKCE, and a state check.

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

/// Why a sign-in did not finish.
#[derive(Debug, thiserror::Error)]
pub enum SignInError {
    /// The loopback port could not be opened.
    #[error("could not open the sign-in port: {0}")]
    Loopback(#[source] std::io::Error),
    /// The config's consent URL is not a URL.
    #[error("the OAuth consent URL is not valid")]
    InvalidConfig,
    /// The consent page answered with an error code, such as `access_denied`.
    #[error("Google did not authorize the app: {0}")]
    Denied(String),
    /// Google's token answer had no refresh token.
    #[error("Google returned no refresh token")]
    NoRefreshToken,
    /// Exchanging the code failed.
    #[error("could not exchange the sign-in code: {0}")]
    Exchange(#[from] GmailError),
}

/// A sign-in waiting for the browser.
///
/// ```no_run
/// # async fn demo(config: &ub_gmail::OAuthConfig, http: &ub_gmail::HttpClient) -> Result<(), ub_gmail::SignInError> {
/// let sign_in = ub_gmail::SignIn::start(config).await?;
/// // Open `sign_in.consent_url()` in the system browser, then:
/// let grant = sign_in.finish(http).await?;
/// # let _ = grant; Ok(()) }
/// ```
///
/// There is no timeout: drop the [`SignIn::finish`] future to cancel a
/// sign-in the user abandoned, which also closes the port.
pub struct SignIn {
    config: OAuthConfig,
    loopback: Loopback,
    redirect_uri: String,
    consent_url: String,
    state: CsrfToken,
    verifier: PkceCodeVerifier,
}

impl SignIn {
    /// Opens a loopback port on `127.0.0.1` and makes the consent URL: PKCE
    /// S256, a random state, both scopes, offline access, and forced consent
    /// so Google always returns a refresh token.
    pub async fn start(config: &OAuthConfig) -> Result<Self, SignInError> {
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
            config: config.clone(),
            loopback,
            redirect_uri,
            consent_url: url.into(),
            state,
            verifier,
        })
    }

    /// The page to open in the system browser.
    pub fn consent_url(&self) -> &str {
        &self.consent_url
    }

    /// Waits for the browser's redirect, checks its state, and trades the
    /// code for tokens. The account's address is not part of the grant: read
    /// it with `getProfile`.
    pub async fn finish(self, http: &HttpClient) -> Result<Grant, SignInError> {
        let code = match self.loopback.wait(self.state.secret()).await {
            Redirect::Code(code) => code,
            Redirect::Error(error) => return Err(SignInError::Denied(error)),
        };
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
