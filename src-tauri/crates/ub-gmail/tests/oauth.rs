//! Sign-in, refresh, revoke and token storage against the fake's OAuth endpoints.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use ub_fake_gmail::FakeGmail;
use ub_gmail::*;

/// Plays the browser: follows the consent page's redirect to the app's loopback port.
fn browser(http: &reqwest::Client) -> impl FnOnce(&str) + use<> {
    let http = http.clone();
    move |url: &str| {
        let url = url.to_owned();
        tokio::spawn(async move {
            let page = http.get(url).send().await.unwrap().text().await.unwrap();
            assert!(page.contains("You can close this tab"), "{page}");
        });
    }
}

#[tokio::test]
async fn loopback_sign_in_end_to_end() {
    let fake = FakeGmail::start().await;
    fake.add_user(ME);
    let cfg = oauth_config(&fake);
    let http = default_http_client();

    let auth = authorize(&cfg, &http, browser(&http)).await.unwrap();

    let request = fake.last_auth_request().unwrap();
    let param = |name: &str| {
        request
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(param("scope"), Some(GMAIL_SCOPE));
    assert_eq!(param("access_type"), Some("offline"));
    assert_eq!(param("prompt"), Some("consent"));
    assert_eq!(param("code_challenge_method"), Some("S256"));
    assert_eq!(param("client_id"), Some("test-client"));
    assert!(
        param("redirect_uri")
            .unwrap()
            .starts_with("http://127.0.0.1:")
    );

    // The new tokens work, and the email comes from getProfile.
    let tokens = Arc::new(OAuthTokens::with_access_token(
        cfg,
        http.clone(),
        auth.refresh_token.0.clone(),
        auth.access_token,
    ));
    let gmail = Gmail::new(fake.base_url(), "unknown-yet", http, tokens);
    assert_eq!(gmail.get_profile().await.unwrap().email_address, ME);
    assert_eq!(fake.count("POST", "/token"), 1);

    let store = MemoryTokenStore::default();
    store
        .store_refresh_token(ME, &auth.refresh_token.0)
        .unwrap();
    assert_eq!(
        store.load_refresh_token(ME).unwrap(),
        Some(auth.refresh_token.0)
    );
}

#[tokio::test]
async fn sign_in_rejects_a_wrong_state() {
    let fake = FakeGmail::start().await;
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
async fn sign_in_reports_a_denied_consent() {
    let fake = FakeGmail::start().await;
    let http = default_http_client();
    let browser = http.clone();
    let result = authorize(&oauth_config(&fake), &http, move |url: &str| {
        let url = url::Url::parse(url).unwrap();
        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        let get = |k: &str| pairs.iter().find(|(n, _)| n == k).unwrap().1.clone();
        let target = format!(
            "{}?error=access_denied&state={}",
            get("redirect_uri"),
            get("state")
        );
        tokio::spawn(async move {
            let _ = browser.get(target).send().await;
        });
    })
    .await;
    let outcome = result.unwrap_err().outcome;
    assert!(
        matches!(&outcome, Outcome::Rejected { message, .. } if message.contains("access_denied")),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn token_source_caches_and_refreshes_before_expiry() {
    let fake = FakeGmail::start().await;
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
    let (fake, _user, gmail) = setup().await;
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

#[tokio::test]
async fn refresh_maps_invalid_grant() {
    let fake = FakeGmail::start().await;
    let user = fake.add_user(ME);
    let http = default_http_client();
    let cfg = oauth_config(&fake);
    assert!(refresh(&cfg, &http, user.refresh_token()).await.is_ok());
    assert_eq!(
        refresh(&cfg, &http, "1//unknown")
            .await
            .unwrap_err()
            .outcome,
        Outcome::InvalidGrant
    );
}

#[tokio::test]
async fn revoke_kills_the_grant_and_is_idempotent() {
    let fake = FakeGmail::start().await;
    let user = fake.add_user(ME);
    let http = default_http_client();
    let cfg = oauth_config(&fake);

    revoke(&cfg, &http, user.refresh_token()).await.unwrap();
    assert_eq!(
        refresh(&cfg, &http, user.refresh_token())
            .await
            .unwrap_err()
            .outcome,
        Outcome::InvalidGrant
    );
    revoke(&cfg, &http, user.refresh_token()).await.unwrap();
}

#[test]
fn memory_store_round_trip() {
    let store = MemoryTokenStore::default();
    assert_eq!(store.load_refresh_token(ME).unwrap(), None);
    store.store_refresh_token(ME, "one").unwrap();
    store.store_refresh_token(ME, "two").unwrap();
    assert_eq!(
        store.load_refresh_token(ME).unwrap().as_deref(),
        Some("two")
    );
    store.delete_refresh_token(ME).unwrap();
    store.delete_refresh_token(ME).unwrap();
    assert_eq!(store.load_refresh_token(ME).unwrap(), None);
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
