//! Tokens, which `Debug` never prints.

use std::fmt;
use std::time::Duration;

use tokio::time::Instant;

/// A refresh token: the account's lasting grant. Lives in the token store.
#[derive(Clone, PartialEq, Eq)]
pub struct RefreshToken(String);

impl RefreshToken {
    /// Wraps a token read from the store or from Google.
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    /// The token itself, for the token store and Google. Never log it.
    #[must_use]
    pub fn secret(&self) -> &str {
        &self.0
    }

    /// The token itself, moved out, for a token store that keeps it. Never
    /// log it.
    #[must_use]
    pub fn into_secret(self) -> String {
        self.0
    }
}

impl fmt::Debug for RefreshToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RefreshToken(<redacted>)")
    }
}

/// An access token, held in memory only.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessToken {
    secret: String,
    expires_at: Instant,
}

/// Refresh an access token this long before it expires, so a 401 is rare.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// Google's lifetime when a token answer leaves `expires_in` out.
const DEFAULT_LIFETIME: Duration = Duration::from_secs(3600);

impl AccessToken {
    /// A token that expires at `expires_at`.
    pub fn new(secret: impl Into<String>, expires_at: Instant) -> Self {
        Self {
            secret: secret.into(),
            expires_at,
        }
    }

    /// A token Google just issued for `expires_in` seconds.
    pub(crate) fn issued(secret: String, expires_in: Option<u64>) -> Self {
        let lifetime = expires_in.map_or(DEFAULT_LIFETIME, Duration::from_secs);
        Self::new(secret, Instant::now() + lifetime)
    }

    /// The bearer token. Never log it.
    #[must_use]
    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// When Google stops accepting it.
    #[must_use]
    pub const fn expires_at(&self) -> Instant {
        self.expires_at
    }

    /// Whether it is still good for a while: it expires after the refresh margin.
    pub(crate) fn is_fresh(&self) -> bool {
        Instant::now() + REFRESH_MARGIN < self.expires_at
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessToken")
            .field("secret", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// What a finished sign-in gives: the grant to keep, and a first access token.
#[derive(Debug, Clone)]
pub struct Grant {
    /// Goes into the token store.
    pub refresh_token: RefreshToken,
    /// Good for about an hour; seeds [`Tokens`](crate::Tokens).
    pub access_token: AccessToken,
}
