use std::fmt;
use std::time::Duration;

use tokio::time::Instant;

#[derive(Clone, PartialEq, Eq)]
pub struct RefreshToken(String);

impl RefreshToken {
    pub fn new(secret: impl Into<String>) -> Self {
        Self(secret.into())
    }

    #[must_use]
    pub fn secret(&self) -> &str {
        &self.0
    }

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

#[derive(Clone, PartialEq, Eq)]
pub struct AccessToken {
    secret: String,
    expires_at: Instant,
}

const REFRESH_MARGIN: Duration = Duration::from_secs(60);

const DEFAULT_LIFETIME: Duration = Duration::from_secs(3600);

impl AccessToken {
    pub fn new(secret: impl Into<String>, expires_at: Instant) -> Self {
        Self {
            secret: secret.into(),
            expires_at,
        }
    }

    pub(crate) fn issued(secret: String, expires_in: Option<u64>) -> Self {
        let lifetime = expires_in.map_or(DEFAULT_LIFETIME, Duration::from_secs);
        Self::new(secret, Instant::now() + lifetime)
    }

    #[must_use]
    pub fn secret(&self) -> &str {
        &self.secret
    }

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

#[derive(Debug, Clone)]
pub struct Grant {
    pub refresh_token: RefreshToken,
    pub access_token: AccessToken,
}
