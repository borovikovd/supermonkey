//! The HTTP client every call shares, and how a response or a transport
//! failure becomes an [`Outcome`].
//!
//! ## Offline or no response
//!
//! A transport failure (no HTTP status at all) is:
//! - **Offline** when reqwest reports a connect error: DNS failure, connection
//!   refused, no route, a failed TLS handshake, a connect timeout. Not one byte
//!   reached the server, so the call is safe to queue and replay.
//! - **No response** otherwise. The connection was up, so the request may have
//!   been written and acted on: a read timeout, a connection closed or reset
//!   before the answer, a broken body.
//!
//! A stale pooled connection that fails while writing looks to hyper like one
//! that died after the write, so it counts as no response: the safe side.

use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::RETRY_AFTER;
use serde::Deserialize;

use crate::outcome::{GmailError, Outcome};

/// The HTTP client for Gmail, Google's OAuth endpoints, and one-click
/// unsubscribe. Cheap to clone: clones share the connection pools.
#[derive(Debug, Clone)]
pub struct HttpClient {
    /// Google's endpoints.
    google: reqwest::Client,
    /// A list's unsubscribe URL, which comes from mail anyone can send: it
    /// follows no redirects, so an `https` URL can't bounce the POST to a
    /// plain `http` or local address.
    lists: reqwest::Client,
}

/// The HTTP client could not start (its TLS backend failed to load).
#[derive(Debug, thiserror::Error)]
#[error("could not start the HTTP client: {0}")]
pub struct HttpClientError(#[source] reqwest::Error);

/// How long connecting may take before the call counts as offline.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the server may go quiet mid-answer before it counts as no response.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

impl HttpClient {
    /// A client with the connect and read timeouts the outcome rule relies on.
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

    /// The client for Google's endpoints.
    pub(crate) const fn inner(&self) -> &reqwest::Client {
        &self.google
    }

    /// The client for a mailing list's one-click unsubscribe URL.
    pub(crate) const fn lists(&self) -> &reqwest::Client {
        &self.lists
    }
}

/// Sends a request, classifying a transport failure.
pub(crate) async fn send(
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, GmailError> {
    request.send().await.map_err(|e| from_transport(&e))
}

/// Classifies a transport failure by the rule in the module docs. The detail
/// leaves out the URL, which can carry ids and search queries.
pub(crate) fn from_transport(err: &reqwest::Error) -> GmailError {
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

/// A response read to its end.
pub(crate) struct ReadResponse {
    pub(crate) status: StatusCode,
    retry_after: Option<Duration>,
    pub(crate) body: Vec<u8>,
}

impl ReadResponse {
    /// Reads the whole body.
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

    /// A response's status and `Retry-After`, its body unread: for a server
    /// that is not Google's, whose body is neither needed nor trusted.
    pub(crate) fn status_only(response: &reqwest::Response) -> Self {
        Self {
            status: response.status(),
            retry_after: retry_after(response),
            body: Vec::new(),
        }
    }

    /// Parses a 2xx body. One that does not parse is no clear answer.
    pub(crate) fn json<T: for<'de> Deserialize<'de>>(&self, what: &str) -> Result<T, GmailError> {
        serde_json::from_slice(&self.body).map_err(|e| parse_error(what, &e))
    }

    /// Classifies a status that is not a success, with its Google error body.
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

/// A response's `Retry-After` in seconds.
fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
        .map(Duration::from_secs)
}

/// A body that did not parse, described without quoting it: serde's message
/// can repeat a value from the body, which may be mail content or a token.
pub(crate) fn parse_error(what: &str, err: &serde_json::Error) -> GmailError {
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

/// `error.message` and the first `error.errors[].reason` of a Google API error.
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
