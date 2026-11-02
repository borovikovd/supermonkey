//! Sync, the op queue, and Gmail error handling against the fake Gmail server
//! (SPEC Testing → Fake Gmail).

use std::sync::Arc;
use std::time::{Duration, Instant};

use ub_fake_gmail::{FakeGmail, FakeUser, Fault, Matcher, NewMessage};
use ub_gmail::{MemoryTokenStore, OAuthConfig, QuotaConfig, TokenStore};
use ubermensch_lib::db::ReadPool;
use ubermensch_lib::engine::{Config, Engine, EngineRef};
use ubermensch_lib::model::*;
use ubermensch_lib::writer::Writer;

const ME: &str = "me@example.com";

struct Harness {
    fake: FakeGmail,
    user: FakeUser,
    engine: EngineRef,
    writer: Writer,
    reads: ReadPool,
    _dir: tempfile::TempDir,
    n: std::sync::atomic::AtomicI64,
}

impl Harness {
    async fn start(seed: impl FnOnce(&FakeUser)) -> Harness {
        let fake = FakeGmail::start().await;
        let user = fake.add_user(ME);
        user.set_display_name("Me");
        seed(&user);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let (etx, erx) = tokio::sync::mpsc::unbounded_channel();
        let writer = Writer::start(path.clone(), etx).unwrap();
        let reads = ReadPool::new(&path, 2);
        let config = Config {
            gmail_base: fake.base_url(),
            oauth: OAuthConfig {
                client_id: "test".into(),
                client_secret: "test".into(),
                auth_url: fake.auth_url(),
                token_url: fake.token_url(),
                revoke_url: fake.revoke_url(),
            },
            quota: QuotaConfig::default(),
        };
        let store = Arc::new(MemoryTokenStore::default());
        store.store_refresh_token(ME, user.refresh_token()).unwrap();
        writer
            .call(|ctx| ubermensch_lib::accounts::create(ctx, ME))
            .await
            .unwrap();
        let engine = Engine::new(
            None,
            writer.clone(),
            reads.clone(),
            dir.path().to_path_buf(),
            config,
            store,
        );
        engine.run(erx);
        Harness {
            fake,
            user,
            engine,
            writer,
            reads,
            _dir: dir,
            n: std::sync::atomic::AtomicI64::new(0),
        }
    }

    fn q<T: rusqlite::types::FromSql>(&self, sql: &str) -> T {
        self.reads
            .with(|c| Ok(c.query_row(sql, [], |r| r.get(0))?))
            .unwrap()
    }

    async fn until(&self, what: &str, f: impl Fn(&Harness) -> bool) {
        let start = Instant::now();
        while !f(self) {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn act(&self, a: Action) {
        let n = self.n.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        self.writer.actions(vec![NumberedAction { n, action: a }]);
    }

    fn archive(&self, thread: ThreadId) {
        self.act(Action::Modify(ModifyAction {
            account: 1,
            kind: ModifyKind::Done,
            threads: vec![thread],
            add: vec![],
            remove: vec!["INBOX".into()],
            snooze: None,
            undoable: true,
        }));
    }

    fn poll(&self) {
        if let Some(rt) = self.engine.account(1) {
            rt.poll_kick.notify_one();
        }
    }

    async fn synced(&self, inbox: i64) {
        self.until("connect sync", |h| {
            h.q::<i64>("SELECT count(*) FROM threads WHERE in_inbox = 1") == inbox
                && h.q::<i64>("SELECT count(*) FROM messages WHERE has_body = 0") == 0
        })
        .await;
    }
}

fn seed(u: &FakeUser) {
    let a = u.add_message(NewMessage::inbox(
        "Ada <ada@acme.io>",
        "Q3 plan",
        "Numbers attached.",
    ));
    u.add_message(NewMessage {
        thread_id: Some(a.thread_id.clone()),
        in_reply_to: Some(a.rfc822_message_id.clone()),
        ..NewMessage::inbox("Grace <grace@acme.io>", "Re: Q3 plan", "Looks good.")
    });
    u.add_message(NewMessage {
        html: Some("<p>Hello <b>there</b><script>alert(1)</script></p>".into()),
        text: None,
        ..NewMessage::inbox("Linus <linus@kernel.org>", "Patch", "")
    });
}

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
        .add_message(NewMessage::inbox("Ken <ken@bell.com>", "Unix", "Hi"));
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
        .add_message(NewMessage::inbox("Ken <ken@bell.com>", "Other", "x"));
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
