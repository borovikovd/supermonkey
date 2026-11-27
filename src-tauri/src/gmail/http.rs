use std::time::Duration;

use rand::Rng as _;
use reqwest::header::{CONTENT_TYPE, RETRY_AFTER};
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, Url, redirect};
use serde::Deserialize;
use thiserror::Error;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const LISTS_TIMEOUT: Duration = Duration::from_secs(10);
const SLOWEST_UPLOAD_BYTES_PER_SEC: u64 = 32 * 1024;
const LARGE_UPLOAD_BYTES: usize = 256 * 1024;
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);
const BASE_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

#[derive(Debug, Error)]
#[error("couldn't initialize HTTP client: {0}")]
pub struct HttpClientError(#[from] reqwest::Error);

#[derive(Debug, Clone)]
pub struct HttpClient {
    google: Client,
    uploads: Client,
    lists: Client,
    idle: Duration,
    slowest_upload: u64,
}

impl HttpClient {
    pub fn new() -> Result<Self, HttpClientError> {
        Self::with_limits(IDLE_TIMEOUT, SLOWEST_UPLOAD_BYTES_PER_SEC)
    }

    pub fn with_limits(idle: Duration, slowest_upload: u64) -> Result<Self, HttpClientError> {
        const USER_AGENT: &str = concat!("Supermonkey/", env!("CARGO_PKG_VERSION"));
        let google = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(idle)
            .build()?;
        let uploads = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()?;
        let lists = Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(LISTS_TIMEOUT)
            .redirect(redirect::Policy::none())
            .build()?;
        Ok(Self {
            google,
            uploads,
            lists,
            idle,
            slowest_upload,
        })
    }

    #[must_use]
    pub const fn google(&self) -> &Client {
        &self.google
    }

    #[must_use]
    pub const fn lists(&self) -> &Client {
        &self.lists
    }

    pub fn google_json(&self, method: Method, url: Url, body: Option<Vec<u8>>) -> RequestBuilder {
        let Some(bytes) = body else {
            return self.google.request(method, url);
        };
        let request = if bytes.len() > LARGE_UPLOAD_BYTES {
            let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            let for_size = Duration::from_secs(size / self.slowest_upload.max(1));
            self.uploads
                .request(method, url)
                .timeout(self.idle.saturating_add(for_size))
        } else {
            self.google.request(method, url)
        };
        request.header(CONTENT_TYPE, "application/json").body(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Offline,
    NoResponse,
    Unauthorized,
    InvalidGrant,
    RateLimited {
        retry_after: Option<Duration>,
    },
    ServerError {
        status: u16,
    },
    NotFound,
    Conflict,
    Rejected {
        status: u16,
        reason: Option<String>,
        message: String,
    },
    InvalidRequest(String),
}

impl Outcome {
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Offline | Self::NoResponse | Self::RateLimited { .. } | Self::ServerError { .. }
        )
    }

    #[must_use]
    pub fn is_missing_scope(&self) -> bool {
        matches!(
            self,
            Self::Rejected {
                status: 403,
                reason: Some(r),
                ..
            } if matches!(
                r.as_str(),
                "insufficientPermissions" | "ACCESS_TOKEN_SCOPE_INSUFFICIENT"
            )
        )
    }

    #[must_use]
    pub fn is_duplicate_filter(&self) -> bool {
        matches!(
            self,
            Self::Rejected {
                status: 400,
                message,
                ..
            } if message.eq_ignore_ascii_case("Filter already exists")
        )
    }

    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after } => *retry_after,
            Self::Offline
            | Self::NoResponse
            | Self::Unauthorized
            | Self::InvalidGrant
            | Self::ServerError { .. }
            | Self::NotFound
            | Self::Conflict
            | Self::Rejected { .. }
            | Self::InvalidRequest(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{detail}")]
pub struct GmailError {
    pub outcome: Outcome,
    pub detail: String,
}

impl GmailError {
    #[must_use]
    pub fn new(outcome: Outcome, detail: impl Into<String>) -> Self {
        Self {
            outcome,
            detail: detail.into(),
        }
    }

    #[must_use]
    pub const fn is_transient(&self) -> bool {
        self.outcome.is_transient()
    }

    #[must_use]
    pub fn is_missing_scope(&self) -> bool {
        self.outcome.is_missing_scope()
    }

    #[must_use]
    pub fn is_duplicate_filter(&self) -> bool {
        self.outcome.is_duplicate_filter()
    }

    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        self.outcome.retry_after()
    }

    #[must_use]
    pub fn from_transport(err: &reqwest::Error) -> Self {
        if err.is_connect() {
            Self::new(Outcome::Offline, "couldn't reach Gmail")
        } else if err.is_builder() {
            Self::new(Outcome::InvalidRequest(err.to_string()), err.to_string())
        } else {
            Self::new(Outcome::NoResponse, "Gmail did not answer in time")
        }
    }
}

pub(super) struct ReadResponse {
    pub(super) status: StatusCode,
    pub(super) retry_after: Option<Duration>,
    pub(super) body: String,
}

impl ReadResponse {
    pub(super) async fn read(response: Response) -> Result<Self, GmailError> {
        let status = response.status();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let body = response
            .text()
            .await
            .map_err(|e| GmailError::from_transport(&e))?;
        Ok(Self {
            status,
            retry_after,
            body,
        })
    }

    pub(super) fn to_error(&self) -> GmailError {
        let (reason, message) = parse_google_error(&self.body);
        let msg = if message.is_empty() {
            format!("Gmail returned HTTP {}", self.status.as_u16())
        } else {
            message
        };
        match self.status.as_u16() {
            401 => GmailError::new(Outcome::Unauthorized, msg),
            404 => GmailError::new(Outcome::NotFound, msg),
            409 => GmailError::new(Outcome::Conflict, msg),
            429 => GmailError::new(
                Outcome::RateLimited {
                    retry_after: self.retry_after,
                },
                msg,
            ),
            403 if matches!(
                reason.as_deref(),
                Some("rateLimitExceeded" | "userRateLimitExceeded" | "backendError")
            ) =>
            {
                GmailError::new(
                    Outcome::RateLimited {
                        retry_after: self.retry_after,
                    },
                    msg,
                )
            }
            code @ 500..=599 => GmailError::new(Outcome::ServerError { status: code }, msg),
            code => GmailError::new(
                Outcome::Rejected {
                    status: code,
                    reason,
                    message: msg.clone(),
                },
                msg,
            ),
        }
    }
}

#[derive(Deserialize)]
struct GoogleErrorEnvelope {
    error: Option<GoogleErrorBody>,
}

#[derive(Deserialize)]
struct GoogleErrorBody {
    #[serde(default)]
    message: String,
    #[serde(default)]
    errors: Vec<GoogleErrorItem>,
}

#[derive(Deserialize)]
struct GoogleErrorItem {
    reason: Option<String>,
}

fn parse_google_error(body: &str) -> (Option<String>, String) {
    let Ok(env) = serde_json::from_str::<GoogleErrorEnvelope>(body) else {
        return (None, String::new());
    };
    let Some(err) = env.error else {
        return (None, String::new());
    };
    let reason = err.errors.into_iter().find_map(|i| i.reason);
    (reason, err.message)
}

#[derive(Debug, Default, Clone)]
pub struct Backoff {
    attempt: u32,
}

impl Backoff {
    #[must_use]
    pub const fn new() -> Self {
        Self { attempt: 0 }
    }

    pub const fn reset(&mut self) {
        self.attempt = 0;
    }

    #[must_use]
    pub fn next_delay(&mut self, retry_after: Option<Duration>) -> Duration {
        let exp = 1u64.checked_shl(self.attempt.min(6)).unwrap_or(64);
        self.attempt = self.attempt.saturating_add(1);
        if let Some(hint) = retry_after {
            return hint.min(MAX_RETRY_AFTER);
        }
        let base = BASE_BACKOFF.saturating_mul(u32::try_from(exp).unwrap_or(u32::MAX));
        let capped = base.min(MAX_BACKOFF);
        let factor: f64 = rand::rng().random_range(0.8..=1.2);
        Duration::from_secs_f64((capped.as_secs_f64() * factor).max(0.1)).min(MAX_BACKOFF)
    }
}
