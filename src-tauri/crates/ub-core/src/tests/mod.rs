//! Tests over the whole core: a started [`App`] on a temporary database.

mod actions;
mod clock;
mod writer_channel;

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use tempfile::TempDir;
use ub_domain::{AccountId, Addr, GmailMessageId, GmailThreadId, LabelSet, Millis, ThreadId};
use ub_gmail::{MemoryTokenStore, OAuthConfig, QuotaConfig};
use ub_ipc::{Action, ChannelMessage, NumberedAction};
use ub_store::{FetchedThread, NewMessage, Priority, StoreError, WriteTx};

use crate::activity::Activity;
use crate::clock::SystemClock;
use crate::{App, Config, Deps, GmailConfig, Notification, Platform, PlatformError};

/// A platform that records what the core asked of it.
#[derive(Default)]
pub(crate) struct Recorder {
    pub(crate) badges: Mutex<Vec<Option<u32>>>,
}

impl Platform for Recorder {
    fn open_url(&self, _: &str) -> Result<(), PlatformError> {
        Ok(())
    }

    fn open_sign_in(&self, _: &str) -> Result<(), PlatformError> {
        Ok(())
    }

    fn open_path(&self, _: &Path) -> Result<(), PlatformError> {
        Ok(())
    }

    fn notify(&self, _: &Notification) {}

    fn set_badge(&self, count: Option<u32>) {
        self.badges.lock().push(count);
    }

    fn focus_window(&self) {}
}

/// A started core on a temporary database, with a page connected whose
/// channel messages are kept in order.
pub(crate) struct Harness {
    pub(crate) dir: TempDir,
    pub(crate) app: App,
    pub(crate) activity: Arc<Activity>,
    pub(crate) platform: Arc<Recorder>,
    pub(crate) messages: Arc<Mutex<Vec<ChannelMessage>>>,
    next: i64,
}

impl Harness {
    pub(crate) async fn start() -> Self {
        Self::start_in(tempfile::tempdir().unwrap()).await
    }

    pub(crate) async fn start_in(dir: TempDir) -> Self {
        let activity = Activity::new();
        let platform = Arc::new(Recorder::default());
        let app = App::start(
            &Config::in_data_dir(
                dir.path(),
                true,
                GmailConfig::google(OAuthConfig::google("", ""), QuotaConfig::default()),
            ),
            Deps {
                platform: Arc::<Recorder>::clone(&platform),
                clock: Arc::new(SystemClock),
                tokens: Arc::new(MemoryTokenStore::default()),
                activity: Arc::clone(&activity),
            },
        )
        .unwrap();
        let messages: Arc<Mutex<Vec<ChannelMessage>>> = Arc::default();
        let sink = Arc::clone(&messages);
        let launch = app
            .lifecycle()
            .connect(Box::new(move |m| sink.lock().push(m)))
            .await
            .unwrap();
        Self {
            dir,
            app,
            activity,
            platform,
            messages,
            next: launch.last_action.0,
        }
    }

    /// Runs a job as sync would, and waits for its commit.
    pub(crate) async fn write<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut WriteTx<'_>) -> Result<T, StoreError> + Send + 'static,
    ) -> T {
        self.app
            .writer()
            .call(Priority::Sync, move |tx, _| job(tx))
            .await
            .unwrap()
    }

    /// Numbers actions as the UI does, submits them in one invoke, and
    /// waits until they have committed. Returns their numbers.
    pub(crate) async fn act(&mut self, actions: Vec<Action>) -> Vec<i64> {
        let numbered: Vec<NumberedAction> = actions
            .into_iter()
            .map(|action| {
                self.next += 1;
                NumberedAction {
                    n: ub_domain::ActionNo(self.next),
                    action,
                }
            })
            .collect();
        let ns = numbered.iter().map(|a| a.n.0).collect();
        self.app.actions().submit(numbered);
        self.app.actions().drained().await.unwrap();
        ns
    }

    /// The channel messages since the last call.
    pub(crate) fn take(&self) -> Vec<ChannelMessage> {
        std::mem::take(&mut *self.messages.lock())
    }
}

/// Polls `done` until it holds, failing after a few seconds: for what the
/// core does on its own tasks after a commit.
pub(crate) async fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !done() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "never happened: {what}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

/// A message from `from` to the fixture account at `date`, with `labels`.
pub(crate) fn message(gmail_id: &str, date: i64, from: &str, labels: &[&str]) -> NewMessage {
    NewMessage {
        gmail_id: GmailMessageId::from(gmail_id),
        date: Millis(date),
        from: Addr::named("Sender", from),
        to: vec![Addr::named("A", ACCOUNT_EMAIL)],
        cc: Vec::new(),
        bcc: Vec::new(),
        reply_to: Vec::new(),
        subject: format!("Subject {gmail_id}"),
        snippet: format!("Snippet {gmail_id}"),
        message_id_header: Some(format!("<{gmail_id}@example.com>")),
        in_reply_to: None,
        references: String::new(),
        is_list: false,
        is_draft: false,
        ub_draft: None,
        labels: labels.iter().copied().collect::<LabelSet>(),
        has_attachments: false,
    }
}

/// The fixture account's address.
pub(crate) const ACCOUNT_EMAIL: &str = "a@example.com";

/// The fixture's newest message date; later writes go after it.
pub(crate) const NEWEST: i64 = 1_800_000_000_000;

/// The fixture: one account with 40 threads, newest first by index. The
/// first 25 are in the Inbox; some are starred, some promotions, two each
/// in Trash and Spam, and threads with a reply carry one from the account.
pub(crate) fn seed(tx: &mut WriteTx<'_>) -> Result<(AccountId, Vec<ThreadId>), StoreError> {
    let account = tx.accounts().create(ACCOUNT_EMAIL, "A")?;
    let mut threads = Vec::new();
    for t in 0..40i64 {
        let base = NEWEST - t * 37 * 60_000;
        let count = 1 + t % 3;
        let mut messages = Vec::new();
        for m in 0..count {
            let date = base - (count - 1 - m) * 3_600_000;
            let id = format!("m{t}_{m}");
            let from_me = m % 2 == 1;
            let mut labels: Vec<&str> = Vec::new();
            if from_me {
                labels.push("SENT");
            } else {
                if t < 25 {
                    labels.push("INBOX");
                }
                if t % 5 == 4 {
                    labels.push("CATEGORY_PROMOTIONS");
                }
                if m + 1 == count && t % 3 == 0 {
                    labels.push("UNREAD");
                }
            }
            if m + 1 == count && t % 7 == 2 {
                labels.push("STARRED");
            }
            if (30..32).contains(&t) {
                labels.push("TRASH");
            }
            if (32..34).contains(&t) {
                labels.push("SPAM");
            }
            let from = if from_me {
                ACCOUNT_EMAIL.to_owned()
            } else {
                format!("person{t}@example.org")
            };
            messages.push(message(&id, date, &from, &labels));
        }
        let stored = tx.threads().store(&FetchedThread {
            account,
            gmail_id: GmailThreadId::from(format!("t{t}")),
            history_id: Some("1".to_owned()),
            messages,
            complete: true,
        })?;
        threads.push(stored.thread.unwrap());
    }
    Ok((account, threads))
}
