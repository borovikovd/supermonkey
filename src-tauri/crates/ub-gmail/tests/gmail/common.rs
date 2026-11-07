//! A fake Gmail and a client wired as the app wires it.

use std::sync::Arc;

use ub_fake_gmail::{FakeGmail, FakeUser};
use ub_gmail::{Gmail, HttpClient, OAuthConfig, QuotaConfig, RefreshToken, Scheduler, Tokens};

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

/// A fake with one user, and a client for that user.
pub fn setup() -> (FakeGmail, FakeUser, Gmail) {
    let fake = FakeGmail::start();
    let user = fake.add_user(ME);
    let gmail = client_for(&fake, &user);
    (fake, user, gmail)
}

/// A client for `user` with production limits.
pub fn client_for(fake: &FakeGmail, user: &FakeUser) -> Gmail {
    let http = HttpClient::new().unwrap();
    let tokens = Arc::new(Tokens::new(
        oauth_config(fake),
        http.clone(),
        RefreshToken::new(user.refresh_token()),
    ));
    let scheduler = Scheduler::new(QuotaConfig::default());
    Gmail::new(
        &fake.base_url(),
        http,
        tokens,
        scheduler.account(user.email()),
    )
    .unwrap()
}
