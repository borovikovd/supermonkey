use std::sync::Arc;

use sm_fake_gmail::{FakeGmail, FakeUser};
use sm_gmail::{Gmail, HttpClient, OAuthConfig, QuotaConfig, RefreshToken, Scheduler, Tokens};

pub(crate) const ME: &str = "me@example.com";

pub(crate) fn oauth_config(fake: &FakeGmail) -> OAuthConfig {
    OAuthConfig {
        client_id: "test-client".into(),
        client_secret: "test-secret".into(),
        auth_url: fake.auth_url(),
        token_url: fake.token_url(),
        revoke_url: fake.revoke_url(),
    }
}

pub(crate) fn setup() -> (FakeGmail, FakeUser, Gmail) {
    let fake = FakeGmail::start();
    let user = fake.add_user(ME);
    let gmail = client_for(&fake, &user);
    (fake, user, gmail)
}

pub(crate) fn client_for(fake: &FakeGmail, user: &FakeUser) -> Gmail {
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
