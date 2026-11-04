#![allow(
    dead_code,
    reason = "Shared by several test binaries; each uses a different subset."
)]

use std::sync::Arc;

use ub_fake_gmail::{FakeGmail, FakeUser};
use ub_gmail::{Gmail, OAuthConfig, OAuthTokens, RefreshToken, default_http_client};

pub const ME: &str = "me@example.com";

/// An OAuth config pointing at the fake, with dummy client credentials.
pub fn oauth_config(fake: &FakeGmail) -> OAuthConfig {
    OAuthConfig {
        client_id: "test-client".into(),
        client_secret: "test-secret".into(),
        auth_url: fake.auth_url(),
        token_url: fake.token_url(),
        revoke_url: fake.revoke_url(),
    }
}

/// A fake with one user and a client wired as the app wires it.
pub fn setup() -> (FakeGmail, FakeUser, Gmail) {
    let fake = FakeGmail::start();
    let user = fake.add_user(ME);
    let gmail = client_for(&fake, &user);
    (fake, user, gmail)
}

pub fn client_for(fake: &FakeGmail, user: &FakeUser) -> Gmail {
    let http = default_http_client();
    let tokens = Arc::new(OAuthTokens::new(
        oauth_config(fake),
        http.clone(),
        RefreshToken(user.refresh_token().to_owned()),
    ));
    Gmail::new(&fake.base_url(), http, tokens)
}

/// A minimal RFC 822 draft carrying the app's local draft id header.
pub fn draft_raw(subject: &str, local_id: &str) -> Vec<u8> {
    format!(
        "From: me@example.com\r\nTo: ann@example.com\r\nSubject: {subject}\r\nX-Ubermensch-Draft: {local_id}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nBody of {subject}\r\n"
    )
    .into_bytes()
}
