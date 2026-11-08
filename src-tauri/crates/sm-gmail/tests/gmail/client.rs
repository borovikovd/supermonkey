//! One-click unsubscribe posts only to `https` and follows no redirect (SPEC
//! Security): a URL taken from mail is untrusted.

use sm_gmail::{Outcome, one_click_unsubscribe};

use crate::common::setup;

#[tokio::test]
async fn one_click_unsubscribe_posts_only_to_https_or_loopback() {
    let (fake, _user, _gmail) = setup();
    let http = sm_gmail::HttpClient::new().unwrap();
    let refused = one_click_unsubscribe(&http, "http://lists.example/unsubscribe")
        .await
        .unwrap_err();
    assert!(matches!(
        refused.outcome(),
        Outcome::Rejected { status: 0, .. }
    ));
    // Debug builds allow loopback http; the fake answers 404 here, which
    // shows the POST was sent.
    let sent = one_click_unsubscribe(&http, &format!("{}/unsubscribe", fake.base_url()))
        .await
        .unwrap_err();
    assert_eq!(sent.outcome(), &Outcome::NotFound);
}

/// A server that answers every connection with `response` and counts them.
async fn answering(
    response: &'static str,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let hits = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = std::sync::Arc::clone(&hits);
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            counted.fetch_add(1, Ordering::SeqCst);
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).await;
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (url, hits)
}

#[tokio::test]
async fn one_click_unsubscribe_follows_no_redirect() {
    use std::sync::atomic::Ordering;

    let (target, target_hits) =
        answering("HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
    let redirect: &'static str = Box::leak(
        format!(
            "HTTP/1.1 307 Temporary Redirect\r\nlocation: {target}/internal\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        )
        .into_boxed_str(),
    );
    let (list, list_hits) = answering(redirect).await;
    let http = sm_gmail::HttpClient::new().unwrap();
    let failed = one_click_unsubscribe(&http, &format!("{list}/unsubscribe"))
        .await
        .unwrap_err();
    assert!(
        matches!(failed.outcome(), Outcome::Rejected { status: 307, .. }),
        "{failed:?}"
    );
    assert_eq!(list_hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        target_hits.load(Ordering::SeqCst),
        0,
        "the redirect was not followed"
    );
}
