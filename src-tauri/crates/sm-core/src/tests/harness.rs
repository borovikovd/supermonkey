use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use sm_domain::{AccountId, Addr, GmailMessageId, GmailThreadId, LabelSet, Millis, ThreadId};
use sm_gmail::{OAuthConfig, QuotaConfig, TokenFile};
use sm_ipc::{Action, ChannelMessage, NumberedAction};
use sm_store::{FetchedThread, NewMessage, Priority, StoreError, WriteTx};
use tempfile::TempDir;

use crate::activity::Activity;
use crate::clock::{Clock, SystemClock};
use crate::{App, Config, Deps, GmailConfig, Notification, Platform, PlatformError};

pub(crate) struct Recorder;

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

    fn set_badge(&self, _: Option<u32>) {}

    fn focus_window(&self) {}
}

pub(crate) struct Harness {
    pub(crate) dir: TempDir,
    pub(crate) app: App,
    pub(crate) activity: Arc<Activity>,
    pub(crate) messages: Arc<Mutex<Vec<ChannelMessage>>>,
    next: i64,
}

impl Harness {
    pub(crate) async fn start() -> Self {
        Self::start_in(tempfile::tempdir().unwrap()).await
    }

    pub(crate) async fn start_in(dir: TempDir) -> Self {
        Self::start_with(dir, Activity::new(), Arc::new(SystemClock)).await
    }

    pub(crate) async fn start_with(
        dir: TempDir,
        activity: Arc<Activity>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let app = App::start(
            Config::in_data_dir(
                dir.path(),
                true,
                GmailConfig::google(OAuthConfig::google("", ""), QuotaConfig::default()),
            ),
            Deps {
                platform: Arc::new(Recorder),
                clock,
                tokens: Arc::new(TokenFile::new(dir.path().join(Config::TOKEN_FILE))),
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
            messages,
            next: launch.last_action.0,
        }
    }

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

    pub(crate) async fn act(&mut self, actions: Vec<Action>) -> Vec<i64> {
        let numbered: Vec<NumberedAction> = actions
            .into_iter()
            .map(|action| {
                self.next += 1;
                NumberedAction {
                    n: sm_domain::ActionNo(self.next),
                    action,
                }
            })
            .collect();
        let ns = numbered.iter().map(|a| a.n.0).collect();
        self.app.actions().submit(numbered);
        self.app.actions().drained().await.unwrap();
        ns
    }

    pub(crate) fn take(&self) -> Vec<ChannelMessage> {
        std::mem::take(&mut *self.messages.lock())
    }
}

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
        sm_draft: None,
        labels: labels.iter().copied().collect::<LabelSet>(),
        has_attachments: false,
    }
}

pub(crate) const ACCOUNT_EMAIL: &str = "a@example.com";

pub(crate) const NEWEST: i64 = 1_800_000_000_000;

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
            history_id: Some(sm_domain::HistoryId(1)),
            messages,
            complete: true,
        })?;
        threads.push(stored.thread.unwrap());
    }
    Ok((account, threads))
}
