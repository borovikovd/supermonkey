//! Every endpoint against the fake, and how failures classify (SPEC Gmail
//! errors). The end-to-end suite covers the app's use of each; these pin the
//! wire format and the outcome table at the crate's edge.

use std::time::Duration;

use ub_domain::{GmailMessageId, LabelId};
use ub_fake_gmail::{Fault, Matcher, NewAttachment, NewMessage};
use ub_gmail::{Format, GmailAttachmentId, Outcome, Priority, ThreadQuery, one_click_unsubscribe};

use crate::common::{ME, draft_raw, setup};

const P: Priority = Priority::P2;

#[tokio::test]
async fn reads_the_profile_addresses_and_labels() {
    let (fake, user, gmail) = setup();
    user.add_send_as("alias@example.com", "Alias");
    let travel = user.add_label("Travel");

    let profile = gmail.get_profile(P).await.unwrap();
    assert_eq!(profile.email_address, ME);
    assert_eq!(profile.history_id.0, user.history_id());

    let send_as = gmail.list_send_as(P).await.unwrap();
    assert!(
        send_as
            .iter()
            .any(|s| s.send_as_email == ME && s.is_primary)
    );
    assert!(
        send_as
            .iter()
            .any(|s| s.send_as_email == "alias@example.com")
    );

    let labels = gmail.list_labels(P).await.unwrap();
    assert!(labels.iter().any(|l| l.id.as_str() == "INBOX"));
    assert!(
        labels
            .iter()
            .any(|l| l.id.as_str() == travel && l.name == "Travel")
    );

    let made = gmail.create_label(P, "Receipts").await.unwrap();
    assert_eq!(made.name, "Receipts");
    let again = gmail.create_label(P, "Receipts").await.unwrap_err();
    assert_eq!(again.outcome(), &Outcome::Conflict);
    assert_eq!(fake.count("GET", "*/profile"), 1);
}

#[tokio::test]
async fn reads_threads_messages_history_and_attachments() {
    let (_fake, user, gmail) = setup();
    let start = gmail.get_profile(P).await.unwrap().history_id;
    let added = user.add_message(&NewMessage {
        attachments: vec![NewAttachment {
            filename: "a.bin".into(),
            mime_type: "application/octet-stream".into(),
            data: vec![0, 1, 2, 250],
            content_id: None,
            inline: false,
            served_inline: false,
        }],
        ..NewMessage::inbox("Ann <ann@example.com>", "Hello", "Hi there")
    });

    let page = gmail
        .list_threads(
            P,
            &ThreadQuery {
                label_ids: vec![LabelId::from("INBOX")],
                max_results: Some(10),
                ..ThreadQuery::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.threads.len(), 1);
    let thread_id = &page.threads[0].id;
    assert_eq!(thread_id.as_str(), added.thread_id);

    let thread = gmail
        .get_thread(P, thread_id, Format::Metadata, &["Subject"])
        .await
        .unwrap();
    let message = &thread.messages[0];
    assert_eq!(message.id.as_str(), added.id);
    assert_eq!(message.header("subject"), Some("Hello"));
    assert!(message.internal_date.is_some());
    assert!(message.label_ids.iter().any(|l| l.as_str() == "UNREAD"));

    let headers = gmail
        .get_message_headers(P, &message.id, &["Subject"])
        .await
        .unwrap();
    assert_eq!(headers.header("Subject"), Some("Hello"));

    let full = gmail
        .get_message(P, &message.id, Format::Full)
        .await
        .unwrap();
    let attachment = find_attachment_id(full.payload.as_ref().unwrap()).unwrap();
    let bytes = gmail
        .get_attachment(P, &message.id, &GmailAttachmentId(attachment))
        .await
        .unwrap();
    assert_eq!(bytes, vec![0, 1, 2, 250]);

    let history = gmail.list_history(P, start, None).await.unwrap();
    assert!(history.history_id > start);
    assert!(
        history
            .history
            .iter()
            .flat_map(|h| &h.messages_added)
            .any(|m| m.message.id.as_str() == added.id)
    );

    gmail
        .batch_modify(
            P,
            std::slice::from_ref(&message.id),
            &[],
            &[LabelId::from("UNREAD")],
        )
        .await
        .unwrap();
    assert!(
        !user
            .message(&added.id)
            .unwrap()
            .label_ids
            .contains(&"UNREAD".to_owned())
    );
    let trashed = gmail.trash_message(P, &message.id).await.unwrap();
    assert!(trashed.label_ids.iter().any(|l| l.as_str() == "TRASH"));

    let missing = gmail
        .get_message(P, &GmailMessageId::from("nope"), Format::Minimal)
        .await
        .unwrap_err();
    assert_eq!(missing.outcome(), &Outcome::NotFound);
}

/// The first `body.attachmentId` in a payload tree.
fn find_attachment_id(part: &serde_json::Value) -> Option<String> {
    if let Some(id) = part["body"]["attachmentId"].as_str() {
        return Some(id.to_owned());
    }
    part["parts"]
        .as_array()?
        .iter()
        .find_map(find_attachment_id)
}

#[tokio::test]
async fn writes_sends_and_deletes_drafts() {
    let (_fake, user, gmail) = setup();
    let draft = gmail
        .create_draft(P, &draft_raw("First", "local-1"), None)
        .await
        .unwrap();
    let listed = gmail.list_drafts(P, None).await.unwrap();
    assert!(listed.drafts.iter().any(|d| d.id == draft.id));

    let updated = gmail
        .update_draft(P, &draft.id, &draft_raw("Second", "local-1"), None)
        .await
        .unwrap();
    assert_eq!(updated.id, draft.id);
    assert_ne!(updated.message.id, draft.message.id);
    let read = gmail
        .get_draft(P, &draft.id, Format::Metadata)
        .await
        .unwrap();
    assert_eq!(read.message.header("X-Ubermensch-Draft"), Some("local-1"));

    let sent = gmail.send_draft(P, &draft.id).await.unwrap();
    assert!(sent.label_ids.iter().any(|l| l.as_str() == "SENT"));
    let gone = gmail
        .get_draft(P, &draft.id, Format::Minimal)
        .await
        .unwrap_err();
    assert_eq!(gone.outcome(), &Outcome::NotFound);

    let other = gmail
        .create_draft(P, &draft_raw("Third", "local-2"), None)
        .await
        .unwrap();
    gmail.delete_draft(P, &other.id).await.unwrap();
    assert!(user.drafts().is_empty());

    let message = gmail
        .send_message(P, &draft_raw("Plain", "none"))
        .await
        .unwrap();
    assert!(message.label_ids.iter().any(|l| l.as_str() == "SENT"));
}

#[tokio::test]
async fn makes_lists_and_deletes_trash_filters() {
    let (_fake, user, gmail) = setup();
    let made = gmail.create_trash_filter(P, "@spam.example").await.unwrap();
    assert_eq!(made.criteria.from.as_deref(), Some("@spam.example"));
    let filters = gmail.list_filters(P).await.unwrap();
    assert_eq!(filters.len(), 1);
    assert_eq!(
        filters[0].action.add_label_ids,
        vec![LabelId::from("TRASH")]
    );
    assert_eq!(
        filters[0].action.remove_label_ids,
        vec![LabelId::from("INBOX")]
    );
    gmail.delete_filter(P, &made.id).await.unwrap();
    assert_eq!(user.filters().len(), 0);

    // Signed in before the app asked for gmail.settings.basic.
    user.set_settings_scope(false);
    let err = gmail.list_filters(P).await.unwrap_err();
    assert!(err.outcome().is_missing_scope(), "{err}");
}

#[tokio::test]
async fn failures_classify_by_the_outcome_table() {
    let (fake, _user, gmail) = setup();
    gmail.get_profile(P).await.unwrap();
    let profile = || gmail.get_profile(P);

    fake.inject(
        Matcher::api("getProfile").once(),
        Fault::Status {
            status: 429,
            reason: "rateLimitExceeded".into(),
            retry_after_secs: Some(7),
        },
    );
    assert_eq!(
        profile().await.unwrap_err().outcome(),
        &Outcome::RateLimited {
            retry_after: Some(Duration::from_secs(7))
        }
    );
    for reason in ["rateLimitExceeded", "userRateLimitExceeded", "backendError"] {
        fake.inject(
            Matcher::api("getProfile").once(),
            Fault::status(403, reason),
        );
        assert_eq!(
            profile().await.unwrap_err().outcome(),
            &Outcome::RateLimited { retry_after: None },
            "403 {reason}"
        );
    }
    fake.inject(
        Matcher::api("getProfile").once(),
        Fault::status(503, "backendError"),
    );
    assert_eq!(
        profile().await.unwrap_err().outcome(),
        &Outcome::ServerError { status: 503 }
    );
    fake.inject(
        Matcher::api("getProfile").once(),
        Fault::status(400, "invalidArgument"),
    );
    assert!(matches!(
        profile().await.unwrap_err().outcome(),
        Outcome::Rejected { status: 400, reason: Some(r), .. } if r == "invalidArgument"
    ));
    fake.inject(
        Matcher::api("getProfile").once(),
        Fault::status(403, "forbidden"),
    );
    let forbidden = profile().await.unwrap_err();
    assert!(matches!(
        forbidden.outcome(),
        Outcome::Rejected { status: 403, .. }
    ));
    assert!(!forbidden.outcome().is_missing_scope());
    fake.inject(Matcher::api("getProfile").once(), Fault::NoResponse);
    assert_eq!(profile().await.unwrap_err().outcome(), &Outcome::NoResponse);
    fake.inject(Matcher::api("getProfile").once(), Fault::Drop);
    assert_eq!(profile().await.unwrap_err().outcome(), &Outcome::NoResponse);

    fake.set_offline(true).await;
    assert_eq!(profile().await.unwrap_err().outcome(), &Outcome::Offline);
    fake.set_offline(false).await;
    profile().await.unwrap();
}

#[tokio::test]
async fn a_401_refreshes_once_and_an_invalid_grant_signs_out() {
    let (fake, user, gmail) = setup();
    gmail.get_profile(P).await.unwrap();

    user.expire_access_tokens();
    gmail.get_profile(P).await.unwrap();
    assert_eq!(fake.count("POST", "/token"), 2);

    // A 401 the fresh token also gets is unauthorized, after one refresh.
    fake.inject(
        Matcher::api("getProfile").times(2),
        Fault::status(401, "authError"),
    );
    assert_eq!(
        gmail.get_profile(P).await.unwrap_err().outcome(),
        &Outcome::Unauthorized
    );
    assert_eq!(fake.count("POST", "/token"), 3);

    user.set_invalid_grant(true);
    user.expire_access_tokens();
    assert_eq!(
        gmail.get_profile(P).await.unwrap_err().outcome(),
        &Outcome::InvalidGrant
    );
}

#[tokio::test]
async fn one_click_unsubscribe_posts_only_to_https_or_loopback() {
    let (fake, _user, _gmail) = setup();
    let http = ub_gmail::HttpClient::new().unwrap();
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
    let http = ub_gmail::HttpClient::new().unwrap();
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
