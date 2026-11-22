use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::RETRY_AFTER;
use serde::Deserialize;

use crate::outcome::{GmailError, Outcome};

#[derive(Debug, Clone)]
pub struct HttpClient {
    google: reqwest::Client,
    lists: reqwest::Client,
}

#[derive(Debug, thiserror::Error)]
#[error("could not start the HTTP client: {0}")]
pub struct HttpClientError(#[source] reqwest::Error);

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(60);

impl HttpClient {
    pub fn new() -> Result<Self, HttpClientError> {
        let builder = || {
            reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
        };
        Ok(Self {
            google: builder().build().map_err(HttpClientError)?,
            lists: builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(HttpClientError)?,
        })
    }

    pub(crate) const fn inner(&self) -> &reqwest::Client {
        &self.google
    }

    pub(crate) const fn lists(&self) -> &reqwest::Client {
        &self.lists
    }
}

pub(crate) async fn send(
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, GmailError> {
    request.send().await.map_err(|e| from_transport(&e))
}

fn from_transport(err: &reqwest::Error) -> GmailError {
    let (outcome, what) = if err.is_connect() {
        (Outcome::Offline, "connect")
    } else if err.is_builder() {
        (
            Outcome::Rejected {
                status: 0,
                message: "the request could not be built".to_owned(),
                reason: None,
            },
            "build",
        )
    } else if err.is_timeout() {
        (Outcome::NoResponse, "timeout")
    } else if err.is_body() || err.is_decode() {
        (Outcome::NoResponse, "body")
    } else {
        (Outcome::NoResponse, "transport")
    };
    GmailError::with_detail(outcome, what)
}

pub(crate) struct ReadResponse {
    pub(crate) status: StatusCode,
    retry_after: Option<Duration>,
    pub(crate) body: Vec<u8>,
}

impl ReadResponse {
    pub(crate) async fn read(response: reqwest::Response) -> Result<Self, GmailError> {
        let status = response.status();
        let retry_after = retry_after(&response);
        let body = response.bytes().await.map_err(|e| from_transport(&e))?;
        Ok(Self {
            status,
            retry_after,
            body: Vec::from(body),
        })
    }

    pub(crate) fn status_only(response: &reqwest::Response) -> Self {
        Self {
            status: response.status(),
            retry_after: retry_after(response),
            body: Vec::new(),
        }
    }

    pub(crate) fn json<T: for<'de> Deserialize<'de>>(&self, what: &str) -> Result<T, GmailError> {
        serde_json::from_slice(&self.body).map_err(|e| parse_error(what, &e))
    }

    pub(crate) fn to_error(&self) -> GmailError {
        let code = self.status.as_u16();
        let retry_after = self.retry_after;
        let (message, reason) = google_error(&self.body);
        let outcome = match code {
            401 => Outcome::Unauthorized,
            404 => Outcome::NotFound,
            409 => Outcome::Conflict,
            429 => Outcome::RateLimited { retry_after },
            403 if matches!(
                reason.as_deref(),
                Some("rateLimitExceeded" | "userRateLimitExceeded" | "backendError")
            ) =>
            {
                Outcome::RateLimited { retry_after }
            }
            500..=599 => Outcome::ServerError { status: code },
            _ => Outcome::Rejected {
                status: code,
                message: message.unwrap_or_else(|| self.status.to_string()),
                reason,
            },
        };
        outcome.into()
    }
}

fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
        .map(Duration::from_secs)
}

fn parse_error(what: &str, err: &serde_json::Error) -> GmailError {
    GmailError::with_detail(
        Outcome::NoResponse,
        format!(
            "{what}: {:?} error at line {} column {}",
            err.classify(),
            err.line(),
            err.column()
        ),
    )
}

fn google_error(body: &[u8]) -> (Option<String>, Option<String>) {
    #[derive(Deserialize)]
    struct Body {
        error: Error,
    }
    #[derive(Deserialize)]
    struct Error {
        message: Option<String>,
        #[serde(default)]
        errors: Vec<Reason>,
    }
    #[derive(Deserialize)]
    struct Reason {
        reason: Option<String>,
    }
    match serde_json::from_slice::<Body>(body) {
        Ok(Body { error }) => (
            error.message,
            error.errors.into_iter().next().and_then(|r| r.reason),
        ),
        Err(_) => (None, None),
    }
}
