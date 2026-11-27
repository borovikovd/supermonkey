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

#[test]
fn only_a_filter_that_trashes_all_mail_from_a_sender_counts_as_a_block() {
    let filters: Vec<supermonkey_lib::gmail::Filter> = serde_json::from_value(serde_json::json!([
        {
            "id": "block",
            "criteria": { "from": "news@news.test" },
            "action": { "addLabelIds": ["TRASH"], "removeLabelIds": ["INBOX"] }
        },
        {
            "id": "narrow-subject",
            "criteria": { "from": "boss@corp.test", "subject": "lunch menu" },
            "action": { "addLabelIds": ["TRASH"] }
        },
        {
            "id": "narrow-attachment",
            "criteria": { "from": "scans@corp.test", "hasAttachment": true },
            "action": { "addLabelIds": ["TRASH"] }
        },
        {
            "id": "label-only",
            "criteria": { "from": "list@corp.test" },
            "action": { "addLabelIds": ["Label_1"] }
        },
        {
            "id": "no-sender",
            "criteria": { "query": "unsubscribe" },
            "action": { "addLabelIds": ["TRASH"] }
        }
    ]))
    .unwrap();
    let blocked: Vec<&str> = filters
        .iter()
        .filter_map(supermonkey_lib::gmail::Filter::blocked_sender)
        .collect();
    assert_eq!(blocked, vec!["news@news.test"]);
}

fn serve_once<F, Fut>(listener: TcpListener, respond: F)
where
    F: FnOnce(TcpStream, Vec<u8>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut request = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let Ok(n) = stream.read(&mut buf).await else {
                return;
            };
            request.extend_from_slice(&buf[..n]);
            let Some(head_end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                if n == 0 {
                    return;
                }
                continue;
            };
            let head = String::from_utf8_lossy(&request[..head_end]).to_ascii_lowercase();
            let length: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            if n == 0 || request.len() >= head_end + 4 + length {
                break;
            }
        }
        respond(stream, request).await;
    });
}

#[tokio::test]
async fn a_download_that_keeps_arriving_outlasts_the_idle_limit_and_a_stalled_one_does_not() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let steady = format!("http://{}/", listener.local_addr().unwrap());
    serve_once(listener, |mut stream, _| async move {
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 60\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
        for _ in 0..6 {
            tokio::time::sleep(Duration::from_millis(150)).await;
            stream.write_all(b"0123456789").await.unwrap();
            stream.flush().await.unwrap();
        }
    });
    let http = HttpClient::with_limits(Duration::from_millis(400), 1).unwrap();
    let body = http
        .google()
        .get(&steady)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(
        body.len(),
        60,
        "900 ms of steady arrival under a 400 ms idle limit"
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stalled = format!("http://{}/", listener.local_addr().unwrap());
    serve_once(listener, |mut stream, _| async move {
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-length: 60\r\nconnection: close\r\n\r\n0123456789",
            )
            .await
            .unwrap();
        stream.flush().await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
    });
    let res = http
        .google()
        .get(&stalled)
        .send()
        .await
        .unwrap()
        .bytes()
        .await;
    assert!(
        res.is_err(),
        "nothing for longer than the idle limit ends the read"
    );
}

#[tokio::test]
async fn a_large_upload_is_given_time_for_its_size_and_a_small_request_is_not() {
    let slow_answer = |mut stream: TcpStream, _request: Vec<u8>| async move {
        tokio::time::sleep(Duration::from_millis(900)).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}")
            .await
            .unwrap();
        stream.flush().await.unwrap();
    };
    let http = HttpClient::with_limits(Duration::from_millis(300), 512 * 1024).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    serve_once(listener, slow_answer);
    let large = vec![b'a'; 1024 * 1024];
    let res = http
        .google_json(reqwest::Method::POST, url.parse().unwrap(), Some(large))
        .send()
        .await;
    assert!(
        res.is_ok_and(|r| r.status().is_success()),
        "1 MB at no less than 512 KB/s may take 2 s beyond the idle limit"
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    serve_once(listener, slow_answer);
    let res = http
        .google_json(
            reqwest::Method::POST,
            url.parse().unwrap(),
            Some(b"{}".to_vec()),
        )
        .send()
        .await;
    assert!(
        res.is_err(),
        "a small request still waits no longer than the idle limit"
    );
}
