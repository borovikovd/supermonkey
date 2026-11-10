//! OAuth logic the end-to-end suite can't observe: the state check, a
//! connection that sends nothing, one refresh shared by concurrent 401s, and
//! secrets kept out of debug output.

use std::time::Duration;

use sm_fake_gmail::FakeGmail;
use sm_gmail::{AccessToken, HttpClient, OAuthConfig, Priority, RefreshToken, SignIn, SignInError};
use tokio::io::AsyncWriteExt as _;

use crate::common::{ME, oauth_config, setup};

/// The redirect URI the consent URL names.
fn redirect_uri(consent_url: &str) -> String {
    url::Url::parse(consent_url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "redirect_uri")
        .unwrap()
        .1
        .into_owned()
}

#[tokio::test]
async fn a_wrong_state_is_refused_and_the_sign_in_goes_on() {
    let fake = FakeGmail::start();
    // The user only has to exist: nothing here uses its handle.
    _ = fake.add_user(ME);
    let http = HttpClient::new().unwrap();
    let sign_in = SignIn::start(oauth_config(&fake)).await.unwrap();
    let consent = sign_in.consent_url().to_owned();
    let redirect = redirect_uri(&consent);
    let finishing = tokio::spawn(async move { sign_in.finish(&http).await });

    // A page that knows the port sends a forged redirect first.
    let forged = reqwest::get(format!("{redirect}?code=stolen&state=forged"))
        .await
        .unwrap();
    assert_eq!(forged.status().as_u16(), 400);
    assert!(
        !finishing.is_finished(),
        "the forged state ended the sign-in"
    );
    assert_eq!(fake.count("POST", "/token"), 0);

    // The user's own consent still completes it.
    assert!(reqwest::get(consent).await.unwrap().status().is_success());
    let grant = tokio::time::timeout(Duration::from_secs(5), finishing)
        .await
        .expect("the sign-in finishes")
        .unwrap()
        .unwrap();
    assert_ne!(grant.refresh_token.secret(), "");
    assert_eq!(fake.count("POST", "/token"), 1);
}

#[tokio::test]
async fn sign_in_finishes_past_a_connection_that_sends_nothing() {
    let fake = FakeGmail::start();
    // The user only has to exist: nothing here uses its handle.
    _ = fake.add_user(ME);
    let http = HttpClient::new().unwrap();
    let sign_in = SignIn::start(oauth_config(&fake)).await.unwrap();
    let consent = sign_in.consent_url().to_owned();
    let port = url::Url::parse(&redirect_uri(&consent))
        .unwrap()
        .port()
        .unwrap();

    // A browser's speculative connection: open, and silent.
    let mut idle = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    idle.write_all(b"GET").await.unwrap();
    // The fake's consent page redirects straight back with a code.
    let browser = tokio::spawn(async move { reqwest::get(consent).await.map(|r| r.status()) });

    let grant = tokio::time::timeout(Duration::from_secs(5), sign_in.finish(&http))
        .await
        .expect("the idle connection held up the redirect")
        .unwrap();
    assert_ne!(grant.refresh_token.secret(), "");
    assert!(browser.await.unwrap().unwrap().is_success());
    assert_eq!(fake.count("POST", "/token"), 1);
    drop(idle);
}

#[tokio::test]
async fn a_denied_sign_in_says_why() {
    let fake = FakeGmail::start();
    // The user only has to exist: nothing here uses its handle.
    _ = fake.add_user(ME);
    fake.deny_next_sign_in();
    let http = HttpClient::new().unwrap();
    let sign_in = SignIn::start(oauth_config(&fake)).await.unwrap();
    let consent = sign_in.consent_url().to_owned();
    tokio::spawn(async move { reqwest::get(consent).await.map(drop) });
    let result = sign_in.finish(&http).await;
    assert!(
        matches!(&result, Err(SignInError::Denied(code)) if code == "access_denied"),
        "{result:?}"
    );
    assert!(result.unwrap_err().to_string().contains("access_denied"));
    assert_eq!(fake.count("POST", "/token"), 0);
}

#[tokio::test]
async fn concurrent_401s_share_one_refresh() {
    let (fake, _user, gmail) = setup();
    gmail.get_profile(Priority::P1).await.unwrap();
    fake.expire_access_tokens();
    let handles: Vec<_> = (0..6)
        .map(|_| {
            let gmail = gmail.clone();
            tokio::spawn(async move { gmail.get_profile(Priority::P1).await })
        })
        .collect();
    for handle in handles {
        handle.await.unwrap().unwrap();
    }
    assert_eq!(fake.count("POST", "/token"), 2);
}

#[test]
fn secrets_stay_out_of_debug_output() {
    let config = OAuthConfig::google("id", "very-secret");
    assert!(!format!("{config:?}").contains("very-secret"));
    let token = AccessToken::new("ya29.secret", tokio::time::Instant::now());
    assert!(!format!("{token:?}").contains("ya29.secret"));
    assert!(!format!("{:?}", RefreshToken::new("1//secret")).contains("1//secret"));
}
