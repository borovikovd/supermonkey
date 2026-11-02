//! Every Gmail call ends in one Outcome (SPEC: Gmail errors), checked against the fake.

mod common;

use std::time::Duration;

use common::*;
use ub_fake_gmail::{Fault, Matcher};
use ub_gmail::{GmailError, Outcome};

fn outcome<T: std::fmt::Debug>(result: Result<T, GmailError>) -> Outcome {
    result.expect_err("call should fail").outcome
}

#[tokio::test]
async fn refused_connection_is_offline_and_recovers() {
    let (fake, _user, gmail) = setup();
    gmail.get_profile().await.unwrap();

    fake.set_offline(true).await;
    assert_eq!(outcome(gmail.get_profile().await), Outcome::Offline);

    fake.set_offline(false).await;
    assert_eq!(gmail.get_profile().await.unwrap().email_address, ME);
}

#[tokio::test]
async fn dropped_connection_after_send_is_no_response() {
    let (fake, _user, gmail) = setup();
    fake.inject(Matcher::new("GET", "*/profile").once(), Fault::Drop);
    assert_eq!(outcome(gmail.get_profile().await), Outcome::NoResponse);
    assert!(gmail.get_profile().await.is_ok());
}

#[tokio::test]
async fn read_timeout_is_no_response() {
    let (fake, user, _) = setup();
    let http = reqwest::Client::builder()
        .read_timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let tokens = std::sync::Arc::new(ub_gmail::OAuthTokens::new(
        oauth_config(&fake),
        http.clone(),
        user.refresh_token(),
    ));
    let gmail = ub_gmail::Gmail::new(fake.base_url(), ME, http, tokens);
    fake.inject(
        Matcher::new("GET", "*/labels"),
        Fault::Delay(Duration::from_secs(2)),
    );
    assert_eq!(outcome(gmail.list_labels().await), Outcome::NoResponse);
}

#[tokio::test]
async fn expired_token_refreshes_once_and_retries() {
    let (fake, _user, gmail) = setup();
    gmail.get_profile().await.unwrap();
    assert_eq!(fake.count("POST", "/token"), 1);

    fake.expire_access_tokens();
    assert!(gmail.get_profile().await.is_ok());
    assert_eq!(fake.count("POST", "/token"), 2);
    assert_eq!(fake.count("GET", "*/profile"), 3);
}

#[tokio::test]
async fn persistent_401_is_unauthorized_after_one_refresh() {
    let (fake, _user, gmail) = setup();
    fake.inject(
        Matcher::new("GET", "*/profile"),
        Fault::status(401, "authError"),
    );
    assert_eq!(outcome(gmail.get_profile().await), Outcome::Unauthorized);
    assert_eq!(fake.count("GET", "*/profile"), 2);
    assert_eq!(fake.count("POST", "/token"), 2);
}

#[tokio::test]
async fn rejected_refresh_token_is_invalid_grant() {
    let (fake, user, gmail) = setup();
    gmail.get_profile().await.unwrap();
    user.set_invalid_grant(true);
    fake.expire_access_tokens();
    assert_eq!(outcome(gmail.get_profile().await), Outcome::InvalidGrant);
}

#[tokio::test]
async fn rate_limits_classify_with_retry_after() {
    let (fake, _user, gmail) = setup();
    fake.inject(
        Matcher::new("GET", "*/labels").once(),
        Fault::Status {
            status: 429,
            reason: "rateLimitExceeded".into(),
            retry_after_secs: Some(7),
        },
    );
    let err = gmail.list_labels().await.unwrap_err();
    assert_eq!(
        err.outcome,
        Outcome::RateLimited {
            retry_after: Some(Duration::from_secs(7))
        }
    );
    assert!(err.is_transient());

    for reason in ["rateLimitExceeded", "userRateLimitExceeded", "backendError"] {
        fake.inject(
            Matcher::new("GET", "*/labels").once(),
            Fault::status(403, reason),
        );
        assert_eq!(
            outcome(gmail.list_labels().await),
            Outcome::RateLimited { retry_after: None },
            "403 {reason}"
        );
    }
}

#[tokio::test]
async fn other_403_is_a_rejection() {
    let (fake, _user, gmail) = setup();
    fake.inject(
        Matcher::new("GET", "*/labels").once(),
        Fault::status(403, "insufficientPermissions"),
    );
    let outcome = outcome(gmail.list_labels().await);
    assert!(
        matches!(outcome, Outcome::Rejected { status: 403, .. }),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn server_errors_classify_as_server_error() {
    let (fake, _user, gmail) = setup();
    for status in [500, 502, 503] {
        fake.inject(
            Matcher::new("GET", "*/profile").once(),
            Fault::status(status, "backendError"),
        );
        assert_eq!(
            outcome(gmail.get_profile().await),
            Outcome::ServerError { status }
        );
    }
}

#[tokio::test]
async fn missing_items_are_not_found() {
    let (_fake, _user, gmail) = setup();
    assert_eq!(
        outcome(
            gmail
                .get_thread("nope", ub_gmail::Format::Minimal, &[])
                .await
        ),
        Outcome::NotFound
    );
    assert_eq!(
        outcome(gmail.get_message("nope", ub_gmail::Format::Minimal).await),
        Outcome::NotFound
    );
    assert_eq!(
        outcome(gmail.get_draft("nope", ub_gmail::Format::Minimal).await),
        Outcome::NotFound
    );
    assert_eq!(
        outcome(
            gmail
                .batch_modify(&["nope".into()], &["STARRED".into()], &[])
                .await
        ),
        Outcome::NotFound
    );
}

#[tokio::test]
async fn duplicate_label_is_conflict() {
    let (_fake, _user, gmail) = setup();
    let label = gmail.create_label("Receipts").await.unwrap();
    assert_eq!(label.name, "Receipts");
    assert_eq!(label.label_type, Some(ub_gmail::LabelType::User));
    assert_eq!(
        outcome(gmail.create_label("receipts").await),
        Outcome::Conflict
    );
}

#[tokio::test]
async fn other_4xx_is_rejected_with_googles_message() {
    let (_fake, user, gmail) = setup();
    let msg = user.add_message(&ub_fake_gmail::NewMessage::inbox("a@x.com", "Hi", "Body"));
    let outcome = outcome(
        gmail
            .batch_modify(&[msg.id], &["Label_404".into()], &[])
            .await,
    );
    assert_eq!(
        outcome,
        Outcome::Rejected {
            status: 400,
            message: "Invalid label: Label_404".into()
        }
    );
}

#[tokio::test]
async fn history_older_than_the_floor_is_not_found() {
    let (_fake, user, gmail) = setup();
    let start = gmail.get_profile().await.unwrap().history_id;
    user.add_message(&ub_fake_gmail::NewMessage::inbox("a@x.com", "Hi", "Body"));
    assert_eq!(
        gmail.list_history(start, None).await.unwrap().history.len(),
        1
    );

    user.set_history_floor(start + 1);
    assert_eq!(
        outcome(gmail.list_history(start, None).await),
        Outcome::NotFound
    );
}

#[tokio::test]
async fn send_with_no_response_still_sends_and_drafts_get_says_404() {
    let (fake, user, gmail) = setup();
    let thread = user.add_message(&ub_fake_gmail::NewMessage::inbox(
        "ann@example.com",
        "Plan",
        "Hi",
    ));
    let draft = gmail
        .create_draft(&draft_raw("Re: Plan", "local-1"), Some(&thread.thread_id))
        .await
        .unwrap();

    fake.inject(
        Matcher::new("POST", "*/drafts/send").once(),
        Fault::NoResponse,
    );
    assert_eq!(
        outcome(gmail.send_draft(&draft.id).await),
        Outcome::NoResponse
    );

    // Settling the send as the outbox does: the draft is gone, so it was sent.
    assert_eq!(
        outcome(gmail.get_draft(&draft.id, ub_gmail::Format::Minimal).await),
        Outcome::NotFound
    );
    let messages = user.thread(&thread.thread_id);
    let sent = messages.last().unwrap();
    assert_eq!(sent.label_ids, vec!["SENT"]);
    assert_eq!(sent.header("Subject"), Some("Re: Plan"));
    assert!(user.drafts().is_empty());
}

#[tokio::test]
async fn create_with_no_response_leaves_a_draft_findable_by_header() {
    let (fake, user, gmail) = setup();
    fake.inject(Matcher::new("POST", "*/drafts").once(), Fault::NoResponse);
    assert_eq!(
        outcome(
            gmail
                .create_draft(&draft_raw("Hello", "local-7"), None)
                .await
        ),
        Outcome::NoResponse
    );

    // The recovery in SPEC: list drafts and read each with format=metadata.
    let drafts = gmail.list_drafts(None).await.unwrap().drafts;
    assert_eq!(drafts.len(), 1);
    let draft = gmail
        .get_draft(&drafts[0].id, ub_gmail::Format::Metadata)
        .await
        .unwrap();
    let headers = draft.message.payload.unwrap()["headers"].clone();
    assert!(
        headers
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["name"] == "X-Ubermensch-Draft" && h["value"] == "local-7")
    );
    assert_eq!(user.drafts().len(), 1);
}
