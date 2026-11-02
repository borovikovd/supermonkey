//! Classifying every Gmail and OAuth call into one [`Outcome`] (SPEC: Gmail errors).
//!
//! ## Offline vs. no response
//!
//! The rule for a transport failure (no HTTP status at all) is:
//! - **Offline**: reqwest reports a connect error (`is_connect`). That covers DNS failure,
//!   connection refused, no route, TLS handshake failure and connect timeouts. Not one byte
//!   of the request reached Gmail, so it is safe to queue and replay.
//! - **`NoResponse`**: every other transport failure. The connection was up, so the request
//!   may have been written and acted on: read timeouts, a connection closed or reset before
//!   the response, and a broken or undecodable response body.
//!
//! A stale pooled connection that fails while writing is reported by hyper the same way as
//! one that dies after the write, so it counts as `NoResponse`. That is the conservative side.

use std::fmt;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, RETRY_AFTER};

/// What happened to a Gmail or OAuth call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No connection, or the request failed before it was sent. Safe to replay.
    Offline,
    /// The request may have been sent, but no response arrived (timeout or dropped connection).
    NoResponse,
    /// 401 that survived one token refresh.
    Unauthorized,
    /// The refresh token was rejected (`invalid_grant`). The account must sign in again.
    InvalidGrant,
    /// 429, or 403 with reason `rateLimitExceeded`, `userRateLimitExceeded` or `backendError`.
    RateLimited { retry_after: Option<Duration> },
    /// A 5xx.
    ServerError { status: u16 },
    /// 404.
    NotFound,
    /// 409, for example `labels.create` with a name that exists.
    Conflict,
    /// Any other 4xx. The only outcome that rolls back an action.
    Rejected { status: u16, message: String },
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::Offline => write!(f, "offline"),
            Outcome::NoResponse => write!(f, "no response"),
            Outcome::Unauthorized => write!(f, "unauthorized"),
            Outcome::InvalidGrant => write!(f, "sign-in expired"),
            Outcome::RateLimited { .. } => write!(f, "rate limited"),
            Outcome::ServerError { status } => write!(f, "server error {status}"),
            Outcome::NotFound => write!(f, "not found"),
            Outcome::Conflict => write!(f, "conflict"),
            Outcome::Rejected { status, message } => write!(f, "rejected ({status}): {message}"),
        }
    }
}

/// The error of every call in this crate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{outcome}{}", detail.as_deref().map(|d| format!(": {d}")).unwrap_or_default())]
pub struct GmailError {
    pub outcome: Outcome,
    /// What went wrong underneath, for logs (e.g. why a body did not parse).
    pub detail: Option<String>,
}

impl GmailError {
    /// Wraps an outcome.
    pub fn new(outcome: Outcome) -> Self {
        GmailError {
            outcome,
            detail: None,
        }
    }

    /// An outcome with the underlying cause kept for logs.
    pub fn with_detail(outcome: Outcome, detail: impl fmt::Display) -> Self {
        GmailError {
            outcome,
            detail: Some(detail.to_string()),
        }
    }

    /// True for the outcomes the spec retries with backoff: rate limits, 5xx and no response.
    /// Whether a particular call may be repeated is the caller's decision.
    pub fn is_transient(&self) -> bool {
        matches!(
            self.outcome,
            Outcome::RateLimited { .. } | Outcome::ServerError { .. } | Outcome::NoResponse
        )
    }

    /// The server's `Retry-After`, if it gave one.
    pub fn retry_after(&self) -> Option<Duration> {
        match self.outcome {
            Outcome::RateLimited { retry_after } => retry_after,
            _ => None,
        }
    }
}

impl From<Outcome> for GmailError {
    fn from(outcome: Outcome) -> Self {
        GmailError {
            outcome,
            detail: None,
        }
    }
}

/// Classifies a transport error by the rule in the module docs.
pub(crate) fn from_transport(err: &reqwest::Error) -> GmailError {
    if err.is_connect() {
        Outcome::Offline.into()
    } else {
        Outcome::NoResponse.into()
    }
}

/// Classifies a non-success HTTP status with its Google-shaped JSON body.
pub(crate) fn from_status(status: StatusCode, headers: &HeaderMap, body: &str) -> GmailError {
    let code = status.as_u16();
    let (message, reason) = google_error(body);
    let retry_after = headers
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
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
            message: message.unwrap_or_else(|| status.to_string()),
        },
    };
    outcome.into()
}

/// Reads `error.message` and the first `error.errors[].reason` of a Google API error body.
fn google_error(body: &str) -> (Option<String>, Option<String>) {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
        return (None, None);
    };
    let error = &json["error"];
    let message = error["message"].as_str().map(str::to_owned);
    let reason = error["errors"][0]["reason"].as_str().map(str::to_owned);
    (message, reason)
}
