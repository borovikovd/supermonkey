//! OAuth logic the end-to-end suite can't observe: the state check, token
//! caching and early refresh, one refresh shared by concurrent 401s, and
//! secrets kept out of debug output.

mod common;

use std::time::Duration;

use common::*;
use ub_fake_gmail::FakeGmail;
use ub_gmail::*;

#[tokio::test]
async fn sign_in_rejects_a_wrong_state() {
    let fake = FakeGmail::start();
    fake.add_user(ME);
    let http = default_http_client();
    let forger = http.clone();
    let result = authorize(&oauth_config(&fake), &http, move |url: &str| {
        let url = url::Url::parse(url).unwrap();
        let redirect = url
            .query_pairs()
            .find(|(k, _)| k == "redirect_uri")
            .unwrap()
            .1
            .into_owned();
        tokio::spawn(async move {
            let _ = forger
                .get(format!("{redirect}?code=stolen&state=forged"))
                .send()
                .await;
        });
    })
    .await;
    assert!(matches!(
        result.unwrap_err().outcome,
        Outcome::Rejected { status: 400, .. }
    ));
    assert_eq!(fake.count("POST", "/token"), 0);
}

#[tokio::test]
async fn token_source_caches_and_refreshes_before_expiry() {
    let fake = FakeGmail::start();
    let user = fake.add_user(ME);
    let http = default_http_client();
    let tokens = OAuthTokens::new(oauth_config(&fake), http.clone(), user.refresh_token());

    let a = tokens.access_token().await.unwrap();
    let b = tokens.access_token().await.unwrap();
    assert_eq!(a, b);
    assert_eq!(fake.count("POST", "/token"), 1);

    // Tokens that expire within the refresh margin are replaced on every use.
    fake.set_token_lifetime(Duration::from_secs(30));
    let c = tokens.refresh_after_401(&b).await.unwrap();
    let d = tokens.access_token().await.unwrap();
    assert_ne!(c, d);
    assert_eq!(fake.count("POST", "/token"), 3);
}

#[tokio::test]
async fn concurrent_401s_share_one_refresh() {
    let (fake, _user, gmail) = setup();
    gmail.get_profile().await.unwrap();
    fake.expire_access_tokens();
    let handles: Vec<_> = (0..6)
        .map(|_| {
            let gmail = gmail.clone();
            tokio::spawn(async move { gmail.get_profile().await })
        })
        .collect();
    for handle in handles {
        handle.await.unwrap().unwrap();
    }
    assert_eq!(fake.count("POST", "/token"), 2);
}

#[test]
fn secrets_stay_out_of_debug_output() {
    let cfg = OAuthConfig::google("id", "very-secret");
    assert!(!format!("{cfg:?}").contains("very-secret"));
    let token = AccessToken {
        token: "ya29.secret".into(),
        expires_at: std::time::Instant::now(),
    };
    assert!(!format!("{token:?}").contains("ya29.secret"));
    assert!(!format!("{:?}", RefreshToken("1//secret".into())).contains("1//secret"));
}
