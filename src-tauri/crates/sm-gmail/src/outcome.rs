//! What a Gmail or OAuth call ended in (SPEC Gmail errors).

use std::fmt;
use std::time::Duration;

/// How a Gmail or OAuth call failed: one variant per row of SPEC's Gmail
/// errors table. Callers match on it; nothing parses error strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No connection, or the request failed before it was sent. Safe to replay.
    Offline,
    /// The request may have been sent, but no clear answer came back: a
    /// timeout, a dropped connection, or a 2xx body that does not parse.
    NoResponse,
    /// A 401 that survived one token refresh.
    Unauthorized,
    /// Google rejected the refresh token (`invalid_grant`): the account must
    /// sign in again.
    InvalidGrant,
    /// 429, or 403 with reason `rateLimitExceeded`, `userRateLimitExceeded`, or
    /// `backendError`. Back off, for `retry_after` when Google gave it.
    RateLimited {
        /// The `Retry-After` header, in whole seconds.
        retry_after: Option<Duration>,
    },
    /// A 5xx.
    ServerError {
        /// The HTTP status.
        status: u16,
    },
    /// A 404: the thread, message, draft, label, or history cursor is gone.
    NotFound,
    /// A 409, as `labels.create` answers for a name that exists.
    Conflict,
    /// Any other 4xx: the only outcome that rolls back an action. Status 0
    /// means the request could not be built or was refused before sending.
    Rejected {
        /// The HTTP status, or 0.
        status: u16,
        /// Google's `error.message`, or the status text.
        message: String,
        /// Google's `error.errors[0].reason` (or the OAuth `error` code).
        reason: Option<String>,
    },
}

impl Outcome {
    /// A 403 `insufficientPermissions`: the grant lacks a scope the method
    /// needs, as for filters on an account signed in before the app asked for
    /// `gmail.settings.basic`. Signing in again fixes it.
    #[must_use]
    pub fn is_missing_scope(&self) -> bool {
        matches!(
            self,
            Self::Rejected { status: 403, reason: Some(reason), .. } if reason == "insufficientPermissions"
        )
    }

    /// The outcomes SPEC retries with back-off: rate limits, 5xx, and no
    /// response. Whether a given call may be repeated is the caller's call.
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::ServerError { .. } | Self::NoResponse
        )
    }

    /// The server's `Retry-After`, if it gave one.
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
            | Self::Rejected { .. } => None,
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Offline => f.write_str("offline"),
            Self::NoResponse => f.write_str("no response"),
            Self::Unauthorized => f.write_str("unauthorized"),
            Self::InvalidGrant => f.write_str("sign-in expired"),
            Self::RateLimited { .. } => f.write_str("rate limited"),
            Self::ServerError { status } => write!(f, "server error {status}"),
            Self::NotFound => f.write_str("not found"),
            Self::Conflict => f.write_str("conflict"),
            Self::Rejected {
                status, message, ..
            } => write!(f, "rejected ({status}): {message}"),
        }
    }
}

/// The error of every Gmail and token call: an [`Outcome`], plus what went
/// wrong underneath for logs. The detail never holds tokens or mail content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{outcome}{}", detail.as_deref().map(|d| format!(": {d}")).unwrap_or_default())]
pub struct GmailError {
    outcome: Outcome,
    detail: Option<String>,
}

impl GmailError {
    /// An error with no further detail.
    #[must_use]
    pub const fn new(outcome: Outcome) -> Self {
        Self {
            outcome,
            detail: None,
        }
    }

    /// An error that keeps its cause for logs.
    pub fn with_detail(outcome: Outcome, detail: impl Into<String>) -> Self {
        Self {
            outcome,
            detail: Some(detail.into()),
        }
    }

    /// What happened.
    #[must_use]
    pub const fn outcome(&self) -> &Outcome {
        &self.outcome
    }

    /// The cause underneath, for logs.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// See [`Outcome::is_transient`].
    #[must_use]
    pub const fn is_transient(&self) -> bool {
        self.outcome.is_transient()
    }

    /// See [`Outcome::retry_after`].
    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        self.outcome.retry_after()
    }
}

impl From<Outcome> for GmailError {
    fn from(outcome: Outcome) -> Self {
        Self::new(outcome)
    }
}
