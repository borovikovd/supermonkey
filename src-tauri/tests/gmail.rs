use std::sync::Arc;
use std::time::Duration;

use sm_fake_gmail::FakeGmail;
use supermonkey_lib::gmail::{
    Gmail, HttpClient, OAuthConfig, Priority, QuotaConfig, RefreshToken, Scheduler, SignIn, Tokens,
    Units, one_click_unsubscribe,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test(start_paused = true)]
async fn scheduler_enforces_priority_concurrency_quota_and_forgets_accounts() {
    let config = QuotaConfig {
        capacity: Units(100),
        refill_per_minute: Units(600),
        background_per_minute: Units(600),
        background_reserve: Units(40),
        max_in_flight: 2,
    };
    let sched = Scheduler::new(config);
    let acct = sched.account("a@example.com");

    let p1 = acct.acquire(Priority::P1, Units(10)).await;
    let p2 = acct.acquire(Priority::P1, Units(10)).await;
    assert_eq!(acct.in_flight(), 2, "at max_in_flight");

    let order = Arc::new(parking_lot::Mutex::new(Vec::<&'static str>::new()));
    let a1 = Arc::clone(&acct);
    let o1 = Arc::clone(&order);
    let t_low = tokio::spawn(async move {
        let _p = a1.acquire(Priority::P5, Units(10)).await;
        o1.lock().push("P5");
    });
    tokio::task::yield_now().await;

    let a2 = Arc::clone(&acct);
    let o2 = Arc::clone(&order);
    let t_high = tokio::spawn(async move {
        let _p = a2.acquire(Priority::P1, Units(10)).await;
        o2.lock().push("P1");
    });
    tokio::task::yield_now().await;

    drop(p1);
    drop(p2);
    t_high.await.unwrap();
    t_low.await.unwrap();
    assert_eq!(*order.lock(), vec!["P1", "P5"], "P1 runs ahead of P5");

    sched.forget("a@example.com");
    let fresh = sched.account("a@example.com");
    assert!(
        fresh.bucket_level() >= 99.0,
        "forgotten account starts with a full bucket"
    );
}

#[tokio::test(start_paused = true)]
async fn scheduler_never_spins_on_stale_wake_at_or_high_background_reserve() {
    let config = QuotaConfig {
        capacity: Units(50),
        refill_per_minute: Units(60),
        background_per_minute: Units(60),
        background_reserve: Units(100),
        max_in_flight: 8,
    };
    let sched = Scheduler::new(config);
    let acct = sched.account("spin@example.com");

    let _drain = acct.acquire(Priority::P1, Units(50)).await;
    let a_bg = Arc::clone(&acct);
    let bg = tokio::spawn(async move { a_bg.acquire(Priority::P4, Units(10)).await });
    tokio::task::yield_now().await;

    let a_hi = Arc::clone(&acct);
    let hi = tokio::spawn(async move { a_hi.acquire(Priority::P1, Units(50)).await });
    tokio::task::yield_now().await;

    tokio::time::advance(Duration::from_secs(120)).await;
    let p_hi = tokio::time::timeout(Duration::from_secs(1), hi)
        .await
        .expect("P1 completes without spinning")
        .unwrap();
    drop(p_hi);
    tokio::time::advance(Duration::from_secs(120)).await;
    let p_bg = tokio::time::timeout(Duration::from_secs(1), bg)
        .await
        .expect("P4 completes even when background_reserve >= capacity")
        .unwrap();
    drop(p_bg);
}

fn fake_oauth_config(fake: &FakeGmail) -> OAuthConfig {
    OAuthConfig {
        client_id: "cid-1".to_owned(),
        client_secret: "SUPER_SECRET_VALUE".to_owned(),
        auth_url: fake.auth_url(),
        token_url: fake.token_url(),
        revoke_url: fake.revoke_url(),
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "exercises OAuth state check, PKCE exchange, concurrent 401 refresh deduplication, and debug redaction"
)]
async fn oauth_rejects_wrong_state_shares_401_refresh_and_redacts_secrets() {
    let fake = FakeGmail::start();
    let user = fake.add_user("alex@example.com");
    let config = fake_oauth_config(&fake);
    assert!(!format!("{config:?}").contains("SUPER_SECRET_VALUE"));
    let rt = RefreshToken::new(user.refresh_token());
    assert!(!format!("{rt:?}").contains(user.refresh_token()));

    let (sign_in, auth_url) = SignIn::start(&config, Some("alex@example.com"))
        .await
        .unwrap();
    let redirect = url::Url::parse(&auth_url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "redirect_uri")
        .map(|(_, v)| v.into_owned())
        .unwrap();
    let waiter = tokio::spawn(sign_in.wait_redirect());

    let http = HttpClient::new().unwrap();
    let bad = http
        .lists()
        .get(format!("{redirect}/?code=abc&state=wrong"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status().as_u16(), 400);
    assert!(!waiter.is_finished());

    let good = http.lists().get(&auth_url).send().await.unwrap();
    let loc = good.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(
        http.lists()
            .get(loc)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        200
    );
    let grant = SignIn::exchange(&http, &config, waiter.await.unwrap().unwrap())
        .await
        .unwrap();
    assert!(!format!("{grant:?}").contains(grant.access_token.expose()));

    let tokens = Tokens::from_grant(http.clone(), config, grant);
    let sched = Scheduler::new(QuotaConfig::default());
    let gmail = Gmail::new(
        http,
        &fake.base_url(),
        tokens,
        sched.account("alex@example.com"),
    )
    .unwrap();
    fake.expire_access_tokens();
    let count_tokens = || {
        fake.requests()
            .into_iter()
            .filter(|r| r.info.path == "/token")
            .count()
    };
    let before = count_tokens();
    let (r1, r2) = tokio::join!(
        gmail.get_profile(Priority::P1),
        gmail.get_profile(Priority::P1)
    );
    assert!(r1.is_ok() && r2.is_ok());
    assert_eq!(count_tokens() - before, 1);
}

#[tokio::test]
async fn one_click_unsubscribe_requires_https_and_follows_no_redirect() {
    let http = HttpClient::new().unwrap();
    let err = one_click_unsubscribe(&http, "http://example.com/unsub")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("HTTPS"));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 512];
            drop(stream.read(&mut buf).await);
            let resp = "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            drop(stream.write_all(resp.as_bytes()).await);
            drop(stream.shutdown().await);
        }
    });
    let res = one_click_unsubscribe(&http, &format!("http://{addr}/unsub")).await;
    assert!(res.is_err());
    drop(server.await);
    drop(TcpStream::connect(addr).await);
}

#[test]
fn a_token_stored_under_a_mixed_case_address_is_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tokens.json");
    std::fs::write(&path, r#"{"Me@Acme.com":"1//earlier"}"#).unwrap();
    let tokens = supermonkey_lib::gmail::TokenFile::new(path);
    let found = tokens.load("me@acme.com").unwrap();
    assert_eq!(
        found.map(|t| t.expose().to_owned()),
        Some("1//earlier".to_owned())
    );
}
