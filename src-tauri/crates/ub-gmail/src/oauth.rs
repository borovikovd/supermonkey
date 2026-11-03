//! Google OAuth for an installed app: loopback redirect, PKCE, refresh and revoke
//! (SPEC: Accounts, Settings).
//!
//! The `oauth2` crate builds the authorization URL, the PKCE pair and the state. The token
//! calls are plain form posts over reqwest so their errors classify like Gmail calls.

use std::fmt;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use oauth2::basic::BasicClient;
use oauth2::{AuthUrl, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl, Scope};
use reqwest::StatusCode;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use crate::error::{self, GmailError, Outcome, ReadResponse};

/// The only scope the app asks for.
pub const GMAIL_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";

/// Refresh an access token this long before it expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// The OAuth client and Google's endpoints. Debug builds point the URLs at the fake.
#[derive(Clone)]
pub struct OAuthConfig {
    pub client_id: String,
    /// A Desktop client's secret. Google does not treat it as confidential, but it is never logged.
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub revoke_url: String,
}

impl OAuthConfig {
    /// A config for Google's real endpoints.
    pub fn google(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        OAuthConfig {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            revoke_url: "https://oauth2.googleapis.com/revoke".into(),
        }
    }
}

impl fmt::Debug for OAuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("auth_url", &self.auth_url)
            .field("token_url", &self.token_url)
            .field("revoke_url", &self.revoke_url)
            .finish()
    }
}

/// An access token held in memory only.
#[derive(Clone)]
pub struct AccessToken {
    pub token: String,
    pub expires_at: Instant,
}

impl AccessToken {
    /// True if the token expires within the refresh margin.
    pub fn expires_soon(&self) -> bool {
        Instant::now() + REFRESH_MARGIN >= self.expires_at
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessToken")
            .field("token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// The result of a completed sign-in.
#[derive(Debug, Clone)]
pub struct AuthResult {
    /// Goes into the Keychain.
    pub refresh_token: RefreshToken,
    pub access_token: AccessToken,
}

/// A refresh token, redacted in Debug output.
#[derive(Clone, PartialEq, Eq)]
pub struct RefreshToken(pub String);

impl fmt::Debug for RefreshToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RefreshToken(<redacted>)")
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenErrorBody {
    error: String,
    error_description: Option<String>,
}

/// Runs the installed-app sign-in.
///
/// Binds `127.0.0.1` on a free port, calls `open_url` with Google's consent URL (PKCE S256,
/// random state, offline access, forced consent so a refresh token is always returned), waits
/// for the browser's redirect, checks the state and exchanges the code. The account's email
/// is not part of the result: call `getProfile` with the new token.
///
/// There is no timeout. Drop the future to cancel a sign-in the user abandoned.
pub async fn authorize(
    cfg: &OAuthConfig,
    http: &reqwest::Client,
    open_url: impl FnOnce(&str),
) -> Result<AuthResult, GmailError> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|e| rejected(0, format!("cannot bind the loopback port: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| rejected(0, e.to_string()))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/");

    let client = BasicClient::new(ClientId::new(cfg.client_id.clone()))
        .set_auth_uri(AuthUrl::new(cfg.auth_url.clone()).map_err(|e| rejected(0, e.to_string()))?)
        .set_redirect_uri(
            RedirectUrl::new(redirect_uri.clone()).map_err(|e| rejected(0, e.to_string()))?,
        );
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, state) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new(GMAIL_SCOPE.into()))
        .set_pkce_challenge(challenge)
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent")
        .url();

    open_url(url.as_str());
    let code = wait_for_code(&listener, state.secret()).await?;

    let form = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", cfg.client_id.as_str()),
        ("client_secret", cfg.client_secret.as_str()),
        ("code_verifier", verifier.secret().as_str()),
    ];
    let response = token_request(http, &cfg.token_url, &form).await?;
    let refresh_token = response
        .refresh_token
        .ok_or_else(|| rejected(200, "Google returned no refresh token"))?;
    Ok(AuthResult {
        refresh_token: RefreshToken(refresh_token),
        access_token: access_token_from(response.access_token, response.expires_in),
    })
}

/// Trades a refresh token for a new access token. `invalid_grant` maps to
/// [`Outcome::InvalidGrant`].
pub async fn refresh(
    cfg: &OAuthConfig,
    http: &reqwest::Client,
    refresh_token: &RefreshToken,
) -> Result<AccessToken, GmailError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.0.as_str()),
        ("client_id", cfg.client_id.as_str()),
        ("client_secret", cfg.client_secret.as_str()),
    ];
    let response = token_request(http, &cfg.token_url, &form).await?;
    Ok(access_token_from(
        response.access_token,
        response.expires_in,
    ))
}

/// Revokes a token with Google (the whole grant goes with it). A token Google already
/// considers invalid counts as revoked.
pub async fn revoke(
    cfg: &OAuthConfig,
    http: &reqwest::Client,
    token: &str,
) -> Result<(), GmailError> {
    let response = post_form(http, &cfg.revoke_url, &[("token", token)]).await?;
    if response.status().is_success() {
        return Ok(());
    }
    let response = ReadResponse::read(response).await?;
    if let Ok(err) = serde_json::from_str::<TokenErrorBody>(&response.body)
        && err.error == "invalid_token"
    {
        return Ok(());
    }
    Err(response.to_error())
}

async fn post_form(
    http: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> Result<reqwest::Response, GmailError> {
    http.post(url)
        .form(form)
        .send()
        .await
        .map_err(|e| error::from_transport(&e))
}

async fn token_request(
    http: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> Result<TokenResponse, GmailError> {
    let response = ReadResponse::read(post_form(http, url, form).await?).await?;
    let status = response.status;
    if status.is_success() {
        return serde_json::from_str(&response.body).map_err(|e| {
            GmailError::with_detail(Outcome::NoResponse, format!("token response: {e}"))
        });
    }
    // OAuth errors are `{"error": "...", "error_description": "..."}`, not Google API errors.
    if let Ok(err) = serde_json::from_str::<TokenErrorBody>(&response.body) {
        if err.error == "invalid_grant" {
            return Err(Outcome::InvalidGrant.into());
        }
        if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
            let description = err.error_description.unwrap_or_default();
            return Err(rejected(
                status.as_u16(),
                format!("{}: {description}", err.error),
            ));
        }
    }
    Err(response.to_error())
}

fn access_token_from(token: String, expires_in: Option<u64>) -> AccessToken {
    AccessToken {
        token,
        expires_at: Instant::now() + Duration::from_secs(expires_in.unwrap_or(3600)),
    }
}

fn rejected(status: u16, message: impl Into<String>) -> GmailError {
    Outcome::Rejected {
        status,
        message: message.into(),
    }
    .into()
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Ubermensch</title>\
<body style=\"font:16px -apple-system,sans-serif;padding:4em;text-align:center\">\
<p>You are signed in. You can close this tab.</p>";
const FAILED_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Ubermensch</title>\
<body style=\"font:16px -apple-system,sans-serif;padding:4em;text-align:center\">\
<p>Sign-in did not complete. You can close this tab and try again.</p>";

/// Serves the loopback port until the redirect carrying `code` (or `error`) arrives.
/// Other requests, such as `/favicon.ico`, get a 404.
async fn wait_for_code(listener: &TcpListener, expected_state: &str) -> Result<String, GmailError> {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        let Some(target) = read_request_target(&mut stream).await else {
            continue;
        };
        let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
            respond(&mut stream, "400 Bad Request", "").await;
            continue;
        };
        let param = |name: &str| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        };
        let answer = match (param("code"), param("error")) {
            (_, Some(error)) => Err(error),
            (Some(code), None) => Ok(code),
            (None, None) => {
                respond(&mut stream, "404 Not Found", "").await;
                continue;
            }
        };
        if param("state").as_deref() != Some(expected_state) {
            respond(&mut stream, "400 Bad Request", FAILED_PAGE).await;
            return Err(rejected(400, "OAuth state mismatch"));
        }
        return match answer {
            Ok(code) => {
                respond(&mut stream, "200 OK", DONE_PAGE).await;
                Ok(code)
            }
            Err(error) => {
                respond(&mut stream, "200 OK", FAILED_PAGE).await;
                Err(rejected(400, format!("authorization failed: {error}")))
            }
        };
    }
}

/// Reads an HTTP request head and returns its request target (`/path?query`).
async fn read_request_target(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 || buf.len() > 16 * 1024 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    // The request line is `METHOD target HTTP/1.1`.
    head.lines()
        .next()?
        .split_whitespace()
        .nth(1)
        .map(str::to_owned)
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Hands the Gmail client its bearer tokens: caches the access token in memory and
/// refreshes it with the account's refresh token shortly before it expires or after a 401.
/// Concurrent callers share one refresh.
pub struct OAuthTokens {
    cfg: OAuthConfig,
    http: reqwest::Client,
    refresh_token: RefreshToken,
    cached: Mutex<Option<AccessToken>>,
}

impl OAuthTokens {
    /// A source with no cached access token yet.
    pub fn new(cfg: OAuthConfig, http: reqwest::Client, refresh_token: RefreshToken) -> Self {
        OAuthTokens {
            cfg,
            http,
            refresh_token,
            cached: Mutex::new(None),
        }
    }

    /// A source seeded with the access token from sign-in.
    pub fn with_access_token(
        cfg: OAuthConfig,
        http: reqwest::Client,
        refresh_token: RefreshToken,
        access_token: AccessToken,
    ) -> Self {
        OAuthTokens {
            cached: Mutex::new(Some(access_token)),
            ..OAuthTokens::new(cfg, http, refresh_token)
        }
    }

    /// A token that is valid for at least a little while.
    pub async fn access_token(&self) -> Result<String, GmailError> {
        let mut cached = self.cached.lock().await;
        match cached.as_ref() {
            Some(token) if !token.expires_soon() => Ok(token.token.clone()),
            _ => self.refresh_into(&mut cached).await,
        }
    }

    /// Called after Gmail answered 401 to `rejected`. Returns a fresh token, refreshing unless
    /// another caller already replaced `rejected`.
    pub async fn refresh_after_401(&self, rejected: &str) -> Result<String, GmailError> {
        let mut cached = self.cached.lock().await;
        match cached.as_ref() {
            Some(token) if token.token != rejected && !token.expires_soon() => {
                Ok(token.token.clone())
            }
            _ => self.refresh_into(&mut cached).await,
        }
    }

    async fn refresh_into(&self, cached: &mut Option<AccessToken>) -> Result<String, GmailError> {
        let token = refresh(&self.cfg, &self.http, &self.refresh_token).await?;
        Ok(cached.insert(token).token.clone())
    }
}
