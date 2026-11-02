//! Shared harness for tests against the fake Gmail server.

#![allow(clippy::unwrap_used)] // Tests fail loudly on purpose.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use ub_fake_gmail::{FakeGmail, FakeUser, NewMessage};
use ub_gmail::{MemoryTokenStore, OAuthConfig, QuotaConfig, TokenStore};
use ubermensch_lib::db::ReadPool;
use ubermensch_lib::engine::{Config, Engine, EngineRef};
use ubermensch_lib::model::*;
use ubermensch_lib::writer::Writer;

pub const ME: &str = "me@example.com";

pub struct Harness {
    pub fake: FakeGmail,
    pub user: FakeUser,
    pub engine: EngineRef,
    pub writer: Writer,
    pub reads: ReadPool,
    _dir: tempfile::TempDir,
    n: std::sync::atomic::AtomicI64,
}

impl Harness {
    pub async fn start(seed: impl FnOnce(&FakeUser)) -> Harness {
        let fake = FakeGmail::start().await;
        let user = fake.add_user(ME);
        user.set_display_name("Me");
        seed(&user);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let (etx, erx) = tokio::sync::mpsc::unbounded_channel();
        let writer = Writer::start(&path, etx).unwrap();
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

    pub fn q<T: rusqlite::types::FromSql>(&self, sql: &str) -> T {
        self.reads
            .with(|c| Ok(c.query_row(sql, [], |r| r.get(0))?))
            .unwrap()
    }

    pub async fn until(&self, what: &str, f: impl Fn(&Harness) -> bool) {
        let start = Instant::now();
        while !f(self) {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub fn act(&self, a: Action) {
        let n = self.n.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        self.writer.actions(vec![NumberedAction { n, action: a }]);
    }

    pub fn archive(&self, thread: ThreadId) {
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

    pub fn poll(&self) {
        if let Some(rt) = self.engine.account(1) {
            rt.poll_kick.notify_one();
        }
    }

    pub async fn synced(&self, inbox: i64) {
        self.until("connect sync", |h| {
            h.q::<i64>("SELECT count(*) FROM threads WHERE in_inbox = 1") == inbox
                && h.q::<i64>("SELECT count(*) FROM messages WHERE has_body = 0") == 0
        })
        .await;
    }
}

pub fn seed(u: &FakeUser) {
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
