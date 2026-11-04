//! The OAuth client and Google's endpoints.

use std::fmt;

/// Read, send, drafts, labels, archive, and trash; not permanent deletion.
pub const GMAIL_MODIFY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";
/// Gmail filters, which Block makes (SPEC Unsubscribe and block).
pub const GMAIL_SETTINGS_SCOPE: &str = "https://www.googleapis.com/auth/gmail.settings.basic";

/// The OAuth client and the endpoints it talks to. Debug builds point the
/// URLs at the fake.
#[derive(Clone, PartialEq, Eq)]
pub struct OAuthConfig {
    /// The Desktop client's id.
    pub client_id: String,
    /// The Desktop client's secret. Google does not treat it as confidential,
    /// but it never reaches a log: `Debug` hides it.
    pub client_secret: String,
    /// The consent page.
    pub auth_url: String,
    /// The token endpoint.
    pub token_url: String,
    /// The revoke endpoint.
    pub revoke_url: String,
}

impl OAuthConfig {
    /// A client against Google's real endpoints.
    pub fn google(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_owned(),
            token_url: "https://oauth2.googleapis.com/token".to_owned(),
            revoke_url: "https://oauth2.googleapis.com/revoke".to_owned(),
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
