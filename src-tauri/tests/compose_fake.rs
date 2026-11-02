//! Drafts, the outbox, undo send, and `drafts.create` recovery against the
//! fake Gmail server (SPEC Testing → Fake Gmail).

#![expect(clippy::unwrap_used, reason = "Tests fail loudly on purpose.")]

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{Harness, seed};
use ub_fake_gmail::{Fault, Matcher};
use ubermensch_lib::model::*;

fn fast_sends() {
    ubermensch_lib::outbox::DELAY_MS.store(300, Ordering::Relaxed);
}

async fn reply_draft(h: &Harness, id: &str, reply_all: bool) -> (ThreadId, MessageId) {
    let thread: i64 = h.q("SELECT id FROM threads WHERE subject = 'Q3 plan'");
    let target: i64 = h.q(&format!(
        "SELECT id FROM messages WHERE thread_id = {thread} ORDER BY internal_date DESC LIMIT 1"
    ));
    h.act(Action::SaveDraft {
        draft: id.into(),
        save: DraftSave::Create {
            account: 1,
            kind: DraftKind::Reply,
            reply_all,
            thread: Some(thread),
            target: Some(target),
            started_on_latest: true,
        },
    });
    let id2 = id.to_string();
    h.until("draft created", move |h| {
        h.q::<i64>(&format!("SELECT count(*) FROM drafts WHERE id = '{id2}'")) == 1
    })
    .await;
    (thread, target)
}

fn update(h: &Harness, id: &str, body: &str) {
    let d = h
        .reads
        .with(|c| ubermensch_lib::db::queries::draft_state(c, id))
        .unwrap();
    h.act(Action::SaveDraft {
        draft: id.into(),
        save: DraftSave::Update {
            fields: DraftFields {
                from: d.from,
                to: d.to,
                cc: d.cc,
                bcc: d.bcc,
                subject: d.subject,
                body_html: body.into(),
                attachments: d.attachments,
            },
        },
    });
}

#[tokio::test(flavor = "multi_thread")]
async fn reply_fills_addressing_and_sends_on_the_thread() {
    fast_sends();
    let h = Harness::start(seed).await;
    h.synced(2).await;
    reply_draft(&h, "d1", true).await;
    // Reply all to Grace's message: To Grace, Cc Ada? No: Ada is not on Grace's To/Cc;
    // the fake addresses everything to me, so Cc stays empty and To is Grace.
    let to: String = h.q("SELECT to_json FROM drafts WHERE id = 'd1'");
    assert!(to.contains("grace@acme.io"), "{to}");
    let subject: String = h.q("SELECT subject FROM drafts WHERE id = 'd1'");
    assert_eq!(subject, "Re: Q3 plan");
    let quote: String = h.q("SELECT quote_html FROM drafts WHERE id = 'd1'");
    assert!(quote.contains("gmail_quote"));

    update(&h, "d1", "<p>Ship it.</p>");
    h.act(Action::SendDraft { draft: "d1".into() });
    h.until("sent", |h| {
        h.q::<i64>("SELECT count(*) FROM outbox WHERE state = 'sent'") == 1
    })
    .await;
    let sent = h
        .user
        .messages()
        .into_iter()
        .find(|m| m.label_ids.iter().any(|l| l == "SENT"))
        .expect("a sent message");
    let original = h
        .user
        .messages()
        .into_iter()
        .find(|m| m.header("Subject") == Some("Re: Q3 plan"))
        .unwrap();
    assert_eq!(sent.thread_id, original.thread_id);
    assert!(sent.header("In-Reply-To").is_some());
    assert_eq!(sent.header("X-Ubermensch-Draft"), Some("d1"));
    // The reply is stored at once, so it never leaves the thread.
    h.until("sent message stored", |h| {
        h.q::<i64>("SELECT count(*) FROM messages WHERE label_ids LIKE '%SENT%'") == 1
    })
    .await;
    assert!(
        h.user.drafts().is_empty(),
        "Gmail removes the draft as it sends"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn undo_on_a_waiting_send_cancels_it() {
    ubermensch_lib::outbox::DELAY_MS.store(5_000, Ordering::Relaxed);
    let h = Harness::start(seed).await;
    h.synced(2).await;
    reply_draft(&h, "d2", false).await;
    update(&h, "d2", "<p>Hold on.</p>");
    h.act(Action::SendDraft { draft: "d2".into() });
    h.until("waiting", |h| {
        h.q::<i64>("SELECT count(*) FROM outbox WHERE state = 'waiting'") == 1
    })
    .await;
    let entry: i64 = h.q("SELECT id FROM undo_entries WHERE kind = 'send'");
    h.act(Action::Undo { account: 1, entry });
    h.until("row gone", |h| {
        h.q::<i64>("SELECT count(*) FROM outbox") == 0
    })
    .await;
    // The draft remains for the composer and is never discarded as untouched.
    assert_eq!(h.q::<i64>("SELECT count(*) FROM drafts WHERE id = 'd2'"), 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !h.user
            .messages()
            .iter()
            .any(|m| m.label_ids.iter().any(|l| l == "SENT"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn send_with_no_clear_answer_settles_as_sent() {
    fast_sends();
    let h = Harness::start(seed).await;
    h.synced(2).await;
    reply_draft(&h, "d3", false).await;
    update(&h, "d3", "<p>Once.</p>");
    h.fake.inject(
        Matcher::new("POST", "*/drafts/send").once(),
        Fault::NoResponse,
    );
    h.act(Action::SendDraft { draft: "d3".into() });
    h.until("settled as sent", |h| {
        h.q::<i64>("SELECT count(*) FROM outbox WHERE state = 'sent'") == 1
    })
    .await;
    let sent = h
        .user
        .messages()
        .into_iter()
        .filter(|m| m.label_ids.iter().any(|l| l == "SENT"))
        .count();
    assert_eq!(sent, 1, "sent exactly once");
}

#[tokio::test(flavor = "multi_thread")]
async fn draft_create_with_no_answer_does_not_duplicate() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    reply_draft(&h, "d4", false).await;
    h.fake
        .inject(Matcher::new("POST", "*/drafts").once(), Fault::NoResponse);
    update(&h, "d4", "<p>Draft text.</p>");
    h.until("upsert done", |h| {
        h.q::<i64>("SELECT count(*) FROM ops") == 0
            && h.q::<Option<String>>("SELECT gmail_draft_id FROM drafts WHERE id = 'd4'")
                .is_some()
    })
    .await;
    assert_eq!(h.user.drafts().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_send_returns_the_draft() {
    fast_sends();
    let h = Harness::start(seed).await;
    h.synced(2).await;
    reply_draft(&h, "d5", false).await;
    update(&h, "d5", "<p>Nope.</p>");
    h.fake.inject(
        Matcher::new("POST", "*/drafts/send").once(),
        Fault::status(400, "invalidArgument"),
    );
    h.act(Action::SendDraft { draft: "d5".into() });
    h.until("send_failed notice", |h| {
        h.q::<i64>("SELECT count(*) FROM notices WHERE kind = 'send_failed' AND draft_id = 'd5'")
            == 1
    })
    .await;
    assert_eq!(h.q::<i64>("SELECT count(*) FROM outbox"), 0);
    assert_eq!(h.q::<i64>("SELECT count(*) FROM drafts WHERE id = 'd5'"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn untouched_fresh_composer_is_discarded_on_close() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    reply_draft(&h, "d6", false).await;
    h.act(Action::CloseComposer { draft: "d6".into() });
    h.until("discarded", |h| {
        h.q::<i64>("SELECT count(*) FROM drafts WHERE id = 'd6'") == 0
    })
    .await;
    assert!(h.user.drafts().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draft_from_gmail_is_read_with_its_kind() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    // Another client makes a forward draft on the Q3 thread.
    let tokens = std::sync::Arc::new(ub_gmail::OAuthTokens::new(
        ub_gmail::OAuthConfig {
            client_id: "test".into(),
            client_secret: "test".into(),
            auth_url: h.fake.auth_url(),
            token_url: h.fake.token_url(),
            revoke_url: h.fake.revoke_url(),
        },
        ub_gmail::default_http_client(),
        h.user.refresh_token(),
    ));
    let gmail = ub_gmail::Gmail::new(
        h.fake.base_url(),
        common::ME,
        ub_gmail::default_http_client(),
        tokens,
    );
    let thread = h
        .user
        .messages()
        .into_iter()
        .find(|m| m.header("Subject") == Some("Q3 plan"))
        .unwrap()
        .thread_id;
    let raw = b"From: me@example.com\r\nTo: ken@bell.com\r\nSubject: Fwd: Q3 plan\r\nContent-Type: text/html\r\n\r\n<p>FYI</p><div class=\"gmail_quote\">old</div>";
    gmail.create_draft(raw, Some(&thread)).await.unwrap();
    // A poll sees the draft message; drafts reconcile.
    h.poll();
    h.until("draft read", |h| {
        h.q::<i64>("SELECT count(*) FROM drafts") == 1
    })
    .await;
    let kind: String = h.q("SELECT kind FROM drafts");
    assert_eq!(kind, "forward");
    let body: String = h.q("SELECT body_html FROM drafts");
    assert!(
        body.contains("FYI") && !body.contains("old"),
        "quote split off: {body}"
    );
    let quote: String = h.q("SELECT quote_html FROM drafts");
    assert!(quote.contains("old"));
}
