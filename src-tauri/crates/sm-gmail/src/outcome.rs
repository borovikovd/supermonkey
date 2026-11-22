use std::fmt;
use std::time::Duration;

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
        message: String,
        reason: Option<String>,
    },
}

impl Outcome {
    #[must_use]
    pub fn is_missing_scope(&self) -> bool {
        matches!(
            self,
            Self::Rejected { status: 403, reason: Some(reason), .. } if reason == "insufficientPermissions"
        )
    }

    #[must_use]
    pub const fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::ServerError { .. } | Self::NoResponse
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{outcome}{}", detail.as_deref().map(|d| format!(": {d}")).unwrap_or_default())]
pub struct GmailError {
    outcome: Outcome,
    detail: Option<String>,
}

impl GmailError {
    #[must_use]
    pub const fn new(outcome: Outcome) -> Self {
        Self {
            outcome,
            detail: None,
        }
    }

    pub fn with_detail(outcome: Outcome, detail: impl Into<String>) -> Self {
        Self {
            outcome,
            detail: Some(detail.into()),
        }
    }

    #[must_use]
    pub const fn outcome(&self) -> &Outcome {
        &self.outcome
    }

    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    #[must_use]
    pub const fn is_transient(&self) -> bool {
        self.outcome.is_transient()
    }

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
