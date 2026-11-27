use std::fmt;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use parking_lot::Mutex;
use rand::RngCore as _;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;
use tokio::time::Instant;
use url::Url;

use super::http::{GmailError, HttpClient, Outcome};

pub const GMAIL_MODIFY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";
pub const GMAIL_SETTINGS_SCOPE: &str = "https://www.googleapis.com/auth/gmail.settings.basic";
const OAUTH_SCOPES: &str = "https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.settings.basic";

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const DEFAULT_TOKEN_LIFETIME: Duration = Duration::from_secs(3600);
const REFRESH_MARGIN: Duration = Duration::from_secs(60);
const MAX_HEAD: usize = 16 * 1024;
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CONCURRENT_CONNS: usize = 8;

const DONE_PAGE: &str = "<!doctype html><title>Supermonkey</title><body style=\"font-family:-apple-system,sans-serif;text-align:center;margin-top:20vh\"><h2>Signed in</h2><p>You can close this tab and return to Supermonkey.</p></body>";
const FAILED_PAGE: &str = "<!doctype html><title>Supermonkey</title><body style=\"font-family:-apple-system,sans-serif;text-align:center;margin-top:20vh\"><h2>Sign-in did not complete</h2><p>You can close this tab and try again in Supermonkey.</p></body>";

#[derive(Clone, PartialEq, Eq)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub revoke_url: String,
}

impl fmt::Debug for OAuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"[redacted]")
            .field("auth_url", &self.auth_url)
            .field("token_url", &self.token_url)
            .field("revoke_url", &self.revoke_url)
            .finish()
    }
}

impl OAuthConfig {
    #[must_use]
    pub fn google(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            auth_url: GOOGLE_AUTH_URL.to_owned(),
            token_url: GOOGLE_TOKEN_URL.to_owned(),
            revoke_url: GOOGLE_REVOKE_URL.to_owned(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RefreshToken(String);

impl RefreshToken {
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub(super) fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for RefreshToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RefreshToken([redacted])")
    }
}

#[derive(Clone)]
pub struct AccessToken {
    token: String,
    expires_at: Instant,
    margin: Duration,
}

impl AccessToken {
    #[must_use]
    pub fn new(token: impl Into<String>, expires_in: Duration) -> Self {
        let margin = REFRESH_MARGIN.min(expires_in / 2);
        Self {
            token: token.into(),
            expires_at: Instant::now() + expires_in,
            margin,
        }
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.token
    }

    #[must_use]
    pub fn is_fresh(&self) -> bool {
        Instant::now() + self.margin < self.expires_at
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessToken")
            .field("token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .field("margin", &self.margin)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct Grant {
    pub access_token: AccessToken,
    pub refresh_token: RefreshToken,
}

#[derive(Debug, Error)]
pub enum SignInError {
    #[error("Google sign-in is not configured in this build")]
    NotConfigured,
    #[error("invalid OAuth URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("couldn't listen for the sign-in redirect: {0}")]
    Bind(#[from] std::io::Error),
    #[error("Sign-in failed: {0}")]
    Denied(String),
    #[error("sign-in did not return a refresh token")]
    NoRefreshToken,
    #[error("{0}")]
    Token(#[from] GmailError),
}

pub struct SignIn {
    listener: TcpListener,
    redirect_uri: String,
    verifier: String,
    state: String,
}

impl fmt::Debug for SignIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignIn")
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct AuthCallback {
    code: String,
    redirect_uri: String,
    verifier: String,
}

impl fmt::Debug for AuthCallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthCallback")
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

impl SignIn {
    pub async fn start(
        config: &OAuthConfig,
        login_hint: Option<&str>,
    ) -> Result<(Self, String), SignInError> {
        if config.client_id.is_empty() {
            return Err(SignInError::NotConfigured);
        }
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}");
        let verifier = random_token(32);
        let challenge = pkce_challenge(&verifier);
        let state = random_token(16);

        let mut params = vec![
            ("response_type", "code"),
            ("client_id", config.client_id.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("scope", OAUTH_SCOPES),
            ("state", state.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ];
        if let Some(hint) = login_hint {
            params.push(("login_hint", hint));
        }
        let auth_url = String::from(Url::parse_with_params(&config.auth_url, &params)?);

        Ok((
            Self {
                listener,
                redirect_uri,
                verifier,
                state,
            },
            auth_url,
        ))
    }

    pub async fn wait_redirect(self) -> Result<AuthCallback, SignInError> {
        let code = accept_callback(self.listener, self.state).await?;
        Ok(AuthCallback {
            code,
            redirect_uri: self.redirect_uri,
            verifier: self.verifier,
        })
    }

    pub async fn exchange(
        http: &HttpClient,
        config: &OAuthConfig,
        callback: AuthCallback,
    ) -> Result<Grant, SignInError> {
        let form = [
            ("grant_type", "authorization_code"),
            ("code", callback.code.as_str()),
            ("redirect_uri", callback.redirect_uri.as_str()),
            ("client_id", config.client_id.as_str()),
            ("client_secret", config.client_secret.as_str()),
            ("code_verifier", callback.verifier.as_str()),
        ];
        let resp = token_request(http, config, &form).await?;
        let refresh = resp.refresh_token.ok_or(SignInError::NoRefreshToken)?;
        let lifetime = resp
            .expires_in
            .map_or(DEFAULT_TOKEN_LIFETIME, Duration::from_secs);
        Ok(Grant {
            access_token: AccessToken::new(resp.access_token, lifetime),
            refresh_token: RefreshToken::new(refresh),
        })
    }
}

pub struct Tokens {
    http: HttpClient,
    config: OAuthConfig,
    refresh: RefreshToken,
    cached: Mutex<Option<AccessToken>>,
    #[expect(
        clippy::disallowed_types,
        reason = "single-flight token refresh must hold its lock across the OAuth HTTP await"
    )]
    refreshing: tokio::sync::Mutex<()>,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tokens").finish_non_exhaustive()
    }
}

impl Tokens {
    #[must_use]
    #[expect(
        clippy::disallowed_types,
        reason = "single-flight token refresh holds its lock across the OAuth HTTP await"
    )]
    pub const fn new(
        http: HttpClient,
        config: OAuthConfig,
        refresh: RefreshToken,
        initial: Option<AccessToken>,
    ) -> Self {
        Self {
            http,
            config,
            refresh,
            cached: Mutex::new(initial),
            refreshing: tokio::sync::Mutex::const_new(()),
        }
    }

    #[must_use]
    pub fn from_grant(http: HttpClient, config: OAuthConfig, grant: Grant) -> Self {
        Self::new(http, config, grant.refresh_token, Some(grant.access_token))
    }

    fn fresh_cached(&self) -> Option<AccessToken> {
        self.cached
            .lock()
            .as_ref()
            .filter(|t| t.is_fresh())
            .cloned()
    }

    pub async fn access_token(&self) -> Result<AccessToken, GmailError> {
        if let Some(tok) = self.fresh_cached() {
            return Ok(tok);
        }
        let _guard = self.refreshing.lock().await;
        if let Some(tok) = self.fresh_cached() {
            return Ok(tok);
        }
        self.do_refresh().await
    }

    pub async fn refresh_after_401(&self, rejected: &str) -> Result<AccessToken, GmailError> {
        let _guard = self.refreshing.lock().await;
        {
            let mut slot = self.cached.lock();
            if let Some(tok) = slot.as_ref() {
                if tok.expose() != rejected && tok.is_fresh() {
                    return Ok(tok.clone());
                }
                if tok.expose() == rejected {
                    *slot = None;
                }
            }
        }
        self.do_refresh().await
    }

    async fn do_refresh(&self) -> Result<AccessToken, GmailError> {
        let form = [
            ("grant_type", "refresh_token"),
            ("refresh_token", self.refresh.expose()),
            ("client_id", self.config.client_id.as_str()),
            ("client_secret", self.config.client_secret.as_str()),
        ];
        let resp = token_request(&self.http, &self.config, &form).await?;
        let lifetime = resp
            .expires_in
            .map_or(DEFAULT_TOKEN_LIFETIME, Duration::from_secs);
        let token = AccessToken::new(resp.access_token, lifetime);
        *self.cached.lock() = Some(token.clone());
        Ok(token)
    }
}

pub async fn revoke(
    http: &HttpClient,
    config: &OAuthConfig,
    token: &RefreshToken,
) -> Result<(), GmailError> {
    let response = http
        .google()
        .post(&config.revoke_url)
        .form(&[("token", token.expose())])
        .send()
        .await
        .map_err(|e| GmailError::from_transport(&e))?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response
        .text()
        .await
        .map_err(|e| GmailError::from_transport(&e))?;
    if status.as_u16() == 400 && body.contains("invalid_token") {
        return Ok(());
    }
    Err(map_oauth_error(status.as_u16(), &body))
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct OAuthErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

async fn token_request(
    http: &HttpClient,
    config: &OAuthConfig,
    form: &[(&str, &str)],
) -> Result<TokenResponse, GmailError> {
    let response = http
        .google()
        .post(&config.token_url)
        .form(form)
        .send()
        .await
        .map_err(|e| GmailError::from_transport(&e))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| GmailError::from_transport(&e))?;
    if !status.is_success() {
        return Err(map_oauth_error(status.as_u16(), &body));
    }
    serde_json::from_str(&body)
        .map_err(|e| GmailError::new(Outcome::NoResponse, format!("bad OAuth response: {e}")))
}

fn map_oauth_error(status: u16, body: &str) -> GmailError {
    let (code, err_desc) = serde_json::from_str::<OAuthErrorBody>(body).map_or_else(
        |_| (String::new(), String::new()),
        |p| (p.error, p.error_description),
    );
    let desc = if err_desc.is_empty() {
        format!("OAuth returned HTTP {status}")
    } else {
        err_desc
    };
    if code == "invalid_grant" {
        return GmailError::new(Outcome::InvalidGrant, desc);
    }
    match status {
        429 => GmailError::new(Outcome::RateLimited { retry_after: None }, desc),
        500..=599 => GmailError::new(Outcome::ServerError { status }, desc),
        _ => GmailError::new(
            Outcome::Rejected {
                status,
                reason: (!code.is_empty()).then_some(code),
                message: desc.clone(),
            },
            desc,
        ),
    }
}

fn random_token(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

enum CallbackOutcome {
    Code(String),
    Denied(String),
    Ignore,
}

async fn accept_callback(
    listener: TcpListener,
    expected_state: String,
) -> Result<String, SignInError> {
    let mut readers = JoinSet::new();
    loop {
        let can_accept = readers.len() < MAX_CONCURRENT_CONNS;
        tokio::select! {
            accepted = listener.accept(), if can_accept => {
                match accepted {
                    Ok((stream, _)) => {
                        let state = expected_state.clone();
                        readers.spawn(handle_conn(stream, state));
                    }
                    Err(e) => {
                        tracing::debug!(error = %e, "loopback accept failed");
                    }
                }
            }
            Some(done) = readers.join_next(), if !readers.is_empty() => {
                if let Ok(outcome) = done {
                    match outcome {
                        CallbackOutcome::Code(code) => return Ok(code),
                        CallbackOutcome::Denied(err) => return Err(SignInError::Denied(err)),
                        CallbackOutcome::Ignore => {}
                    }
                }
            }
        }
    }
}

async fn handle_conn(mut stream: TcpStream, expected_state: String) -> CallbackOutcome {
    let Ok(head) = read_head(&mut stream).await else {
        return CallbackOutcome::Ignore;
    };
    let Some(first_line) = head.lines().next() else {
        return CallbackOutcome::Ignore;
    };
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    if method != "GET" || !target.starts_with('/') {
        write_http(&mut stream, "400 Bad Request", FAILED_PAGE).await;
        return CallbackOutcome::Ignore;
    }
    let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
        write_http(&mut stream, "400 Bad Request", FAILED_PAGE).await;
        return CallbackOutcome::Ignore;
    };
    if url.path() != "/" {
        write_http(&mut stream, "404 Not Found", "").await;
        return CallbackOutcome::Ignore;
    }
    let mut code = None;
    let mut state_ok = false;
    let mut err = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state_ok = v.as_ref() == expected_state,
            "error" => err = Some(v.into_owned()),
            _ => {}
        }
    }
    if !state_ok {
        write_http(&mut stream, "400 Bad Request", FAILED_PAGE).await;
        return CallbackOutcome::Ignore;
    }
    if let Some(err_code) = err {
        write_http(&mut stream, "200 OK", FAILED_PAGE).await;
        return CallbackOutcome::Denied(err_code);
    }
    if let Some(code) = code {
        write_http(&mut stream, "200 OK", DONE_PAGE).await;
        return CallbackOutcome::Code(code);
    }
    write_http(&mut stream, "400 Bad Request", FAILED_PAGE).await;
    CallbackOutcome::Ignore
}

async fn read_head(stream: &mut TcpStream) -> Result<String, ()> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let fut = async {
        loop {
            let n = stream.read(&mut chunk).await.map_err(|_io| ())?;
            if n == 0 || buf.len() + n > MAX_HEAD {
                return Err(());
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                return String::from_utf8(buf).map_err(|_utf8| ());
            }
        }
    };
    tokio::time::timeout(HEAD_TIMEOUT, fut)
        .await
        .map_err(|_to| ())?
}

async fn write_http(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    drop(stream.write_all(response.as_bytes()).await);
    drop(stream.shutdown().await);
}
