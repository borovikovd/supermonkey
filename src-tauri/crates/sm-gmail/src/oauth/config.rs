use std::fmt;

pub const GMAIL_MODIFY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";
pub const GMAIL_SETTINGS_SCOPE: &str = "https://www.googleapis.com/auth/gmail.settings.basic";

#[derive(Clone, PartialEq, Eq)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub revoke_url: String,
}

impl OAuthConfig {
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
