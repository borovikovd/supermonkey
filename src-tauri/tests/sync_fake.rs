//! Sync, the op queue, and Gmail error handling against the fake Gmail server
//! (SPEC Testing → Fake Gmail).

mod common;

use std::time::Duration;

use common::{Harness, seed};
use ub_fake_gmail::{Fault, Matcher, NewMessage};

#[tokio::test(flavor = "multi_thread")]
async fn connect_sync_then_archive_reaches_gmail() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    assert_eq!(h.q::<i64>("SELECT count(*) FROM messages"), 3);
    // Bodies are sanitized at ingest.
    let html: String = h.q("SELECT body_html FROM messages WHERE subject = 'Patch'");
    assert!(html.contains("<b>there</b>") && !html.contains("script"));
    // The Q3 thread is person-to-person, so it lands in Important.
    let split: String = h.q("SELECT s.kind FROM threads t JOIN splits s ON s.id = t.split_id WHERE t.subject = 'Q3 plan'");
    assert_eq!(split, "important");

    let t: i64 = h.q("SELECT id FROM threads WHERE subject = 'Patch'");
    h.archive(t);
    // The list changes before the network returns; Gmail follows.
    h.until("archive applied locally", |h| {
        h.q::<i64>(&format!("SELECT in_inbox FROM threads WHERE id = {t}")) == 0
    })
    .await;
    h.until("op replayed", |h| {
        h.q::<i64>("SELECT count(*) FROM ops") == 0
    })
    .await;
    let msg = h
        .user
        .messages()
        .into_iter()
        .find(|m| m.header("Subject") == Some("Patch"))
        .unwrap();
    assert!(!msg.label_ids.iter().any(|l| l == "INBOX"));
}

#[tokio::test(flavor = "multi_thread")]
async fn poll_picks_up_new_mail_and_remote_changes() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    let added = h
        .user
        .add_message(&NewMessage::inbox("Ken <ken@bell.com>", "Unix", "Hi"));
    h.poll();
    h.until("new mail", |h| {
        h.q::<i64>("SELECT count(*) FROM threads WHERE subject = 'Unix'") == 1
    })
    .await;
    // Archived from another client.
    h.user.modify_labels(&added.id, &[], &["INBOX"]);
    h.poll();
    h.until("remote archive", |h| {
        h.q::<i64>("SELECT in_inbox FROM threads WHERE subject = 'Unix'") == 0
    })
    .await;
    // Done lists it, because this app saw it in the inbox.
    let done: i64 = h.q("SELECT seen_in_inbox FROM threads WHERE subject = 'Unix'");
    assert_eq!(done, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn rejection_rolls_back_and_leaves_a_notice() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    h.fake.inject(
        Matcher::new("POST", "*/messages/batchModify").once(),
        Fault::status(400, "invalidArgument"),
    );
    let t: i64 = h.q("SELECT id FROM threads WHERE subject = 'Patch'");
    h.archive(t);
    h.until("notice", |h| {
        h.q::<i64>("SELECT count(*) FROM notices") == 1
    })
    .await;
    assert_eq!(
        h.q::<i64>(&format!("SELECT in_inbox FROM threads WHERE id = {t}")),
        1
    );
    assert_eq!(h.q::<i64>("SELECT count(*) FROM ops"), 0);
    assert_eq!(h.q::<i64>("SELECT count(*) FROM undo_entries"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limits_and_server_errors_retry() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    h.fake.inject(
        Matcher::new("POST", "*/messages/batchModify").once(),
        Fault::Status {
            status: 429,
            reason: "rateLimitExceeded".into(),
            retry_after_secs: Some(0),
        },
    );
    h.fake.inject(
        Matcher::new("POST", "*/messages/batchModify").once(),
        Fault::status(503, "backendError"),
    );
    let t: i64 = h.q("SELECT id FROM threads WHERE subject = 'Patch'");
    h.archive(t);
    h.until("archive applied locally", |h| {
        h.q::<i64>(&format!("SELECT in_inbox FROM threads WHERE id = {t}")) == 0
    })
    .await;
    h.until("op replayed after retries", |h| {
        h.q::<i64>("SELECT count(*) FROM ops") == 0
    })
    .await;
    assert_eq!(h.q::<i64>("SELECT count(*) FROM notices"), 0);
    let msg = h
        .user
        .messages()
        .into_iter()
        .find(|m| m.header("Subject") == Some("Patch"))
        .unwrap();
    assert!(!msg.label_ids.iter().any(|l| l == "INBOX"));
    assert!(h.fake.count("POST", "*/messages/batchModify") >= 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_queues_and_replays() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    h.fake.set_offline(true).await;
    let t: i64 = h.q("SELECT id FROM threads WHERE subject = 'Patch'");
    h.archive(t);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(h.q::<i64>("SELECT count(*) FROM ops"), 1);
    assert_eq!(
        h.q::<i64>(&format!("SELECT in_inbox FROM threads WHERE id = {t}")),
        0
    );
    h.fake.set_offline(false).await;
    h.engine.account(1).unwrap().ops_kick.notify_one();
    h.until("replay after reconnect", |h| {
        h.q::<i64>("SELECT count(*) FROM ops") == 0
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_action_wins_over_a_poll() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    h.fake.inject(
        Matcher::new("POST", "*/messages/batchModify").once(),
        Fault::Delay(Duration::from_millis(1500)),
    );
    let t: i64 = h.q("SELECT id FROM threads WHERE subject = 'Patch'");
    h.archive(t);
    // A poll while the archive is still pending must not bring the row back.
    h.user
        .add_message(&NewMessage::inbox("Ken <ken@bell.com>", "Other", "x"));
    h.poll();
    h.until("poll ran", |h| {
        h.q::<i64>("SELECT count(*) FROM threads WHERE subject = 'Other'") == 1
    })
    .await;
    assert_eq!(
        h.q::<i64>(&format!("SELECT in_inbox FROM threads WHERE id = {t}")),
        0
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_cursor_runs_a_full_resync() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    let msgs = h.user.messages();
    let patch = msgs
        .iter()
        .find(|m| m.header("Subject") == Some("Patch"))
        .unwrap();
    h.user.modify_labels(&patch.id, &["STARRED"], &[]);
    h.user.set_history_floor(h.user.history_id() + 1);
    h.poll();
    h.until("resync refetched the changed thread", |h| {
        h.q::<i64>("SELECT starred FROM threads WHERE subject = 'Patch'") == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_grant_asks_to_sign_in_again() {
    let h = Harness::start(|_| {}).await;
    h.until("connected", |h| {
        h.q::<i64>("SELECT connected FROM accounts") == 1
    })
    .await;
    h.user.set_invalid_grant(true);
    h.fake.expire_access_tokens();
    h.poll();
    h.until("needs sign-in", |h| {
        h.q::<i64>("SELECT needs_signin FROM accounts") == 1
    })
    .await;
}

fn snooze(h: &Harness, thread: i64, at: i64) {
    h.act(ubermensch_lib::model::Action::Modify(
        ubermensch_lib::model::ModifyAction {
            account: 1,
            kind: ubermensch_lib::model::ModifyKind::Snooze,
            threads: vec![thread],
            add: vec![],
            remove: vec!["INBOX".into()],
            snooze: Some(ubermensch_lib::model::SnoozeChange::Set(at)),
            undoable: true,
        },
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn snooze_leaves_inbox_everywhere_and_wakes_back() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    let t: i64 = h.q("SELECT id FROM threads WHERE subject = 'Patch'");
    let soon = ubermensch_lib::db::now_ms() + 1500;
    snooze(&h, t, soon);
    h.until("snoozed", |h| {
        h.q::<i64>(&format!("SELECT in_inbox FROM threads WHERE id = {t}")) == 0
    })
    .await;
    h.until("Gmail lost INBOX", |h| {
        h.user.messages().iter().any(|m| {
            m.header("Subject") == Some("Patch") && !m.label_ids.iter().any(|l| l == "INBOX")
        })
    })
    .await;
    // Only this app wakes it: INBOX comes back locally and on Gmail.
    h.until("woke", |h| {
        h.q::<i64>(&format!("SELECT in_inbox FROM threads WHERE id = {t}")) == 1
            && h.q::<Option<i64>>(&format!("SELECT snooze_until FROM threads WHERE id = {t}"))
                .is_none()
    })
    .await;
    h.until("Gmail has INBOX again", |h| {
        h.user.messages().iter().any(|m| {
            m.header("Subject") == Some("Patch") && m.label_ids.iter().any(|l| l == "INBOX")
        })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_message_from_outside_clears_a_snooze() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    let t: i64 = h.q("SELECT id FROM threads WHERE subject = 'Patch'");
    let gmail_thread: String = h.q(&format!("SELECT gmail_id FROM threads WHERE id = {t}"));
    snooze(&h, t, ubermensch_lib::db::now_ms() + 3_600_000);
    h.until("op done", |h| {
        h.q::<i64>("SELECT count(*) FROM ops") == 0
            && h.q::<i64>(&format!("SELECT in_inbox FROM threads WHERE id = {t}")) == 0
    })
    .await;
    h.poll();
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Our own label change came back through history: still snoozed.
    assert!(
        h.q::<Option<i64>>(&format!("SELECT snooze_until FROM threads WHERE id = {t}"))
            .is_some()
    );
    h.user.add_message(&NewMessage {
        thread_id: Some(gmail_thread),
        ..NewMessage::inbox("Linus <linus@kernel.org>", "Re: Patch", "v2 attached")
    });
    h.poll();
    h.until("snooze cleared", |h| {
        h.q::<Option<i64>>(&format!("SELECT snooze_until FROM threads WHERE id = {t}"))
            .is_none()
            && h.q::<i64>(&format!("SELECT in_inbox FROM threads WHERE id = {t}")) == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn new_inbox_mail_notifies_unless_muted() {
    let h = Harness::start(seed).await;
    h.synced(2).await;
    h.user
        .add_message(&NewMessage::inbox("Ken <ken@bell.com>", "Ping", "hello"));
    h.poll();
    h.until("notified", |h| h.engine.last_notification.lock().is_some())
        .await;
    let n = h.engine.last_notification.lock().take().unwrap().0;
    assert_eq!(n.title, "Ken");
    assert!(n.thread.is_some());
    // Muted: nothing.
    h.act(ubermensch_lib::model::Action::SetSetting {
        account: Some(1),
        key: "muted".into(),
        value: "on".into(),
    });
    h.user
        .add_message(&NewMessage::inbox("Ken <ken@bell.com>", "Ping 2", "hello"));
    h.poll();
    h.until("second message stored", |h| {
        h.q::<i64>("SELECT count(*) FROM threads WHERE subject = 'Ping 2'") == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.engine.last_notification.lock().is_none());
}
