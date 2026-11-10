//! The E2E server's state: the fake Gmail, the core's lifecycle, app time,
//! and `settle()`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};
use sm_bridge::WebBridge;
use sm_core::activity::{Activity, DEFERRED_KICK};
use sm_core::clock::{Clock as _, ManualClock};
use sm_core::{App, Config, Deps, GmailConfig};
use sm_domain::{GmailThreadId, Millis, ThreadId};
use sm_fake_gmail::{FakeGmail, FakeUser};
use sm_gmail::{OAuthConfig, QuotaConfig, TokenFile};

use crate::platform::RecordingPlatform;

/// How long `advance`, `release`, and a graceful stop wait for the core;
/// `idle` takes its own limit.
pub(crate) const SETTLE_LIMIT: Duration = Duration::from_secs(10);

/// The core, as the test sees it.
pub(crate) enum Core {
    /// Seeding: the database is open and accounts can be added; nothing
    /// talks to Gmail yet. `quota` is the budget it was started with.
    Ready { app: Arc<App>, quota: QuotaConfig },
    /// Launched and served to the page.
    Running(Arc<App>),
    /// Quit or crashed; `start` relaunches it on the same database.
    Down,
}

/// A request to the remote image host or the `evil.test` sink.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Hit {
    pub(crate) method: String,
    pub(crate) path: String,
    /// The request body as text; empty for GETs.
    pub(crate) body: String,
    pub(crate) at: Millis,
}

/// Everything the control API works on. The data folder lives as long as
/// the server.
pub(crate) struct Server {
    pub(crate) fake: FakeGmail,
    /// Fake Gmail users by lower-cased address.
    pub(crate) users: Mutex<HashMap<String, FakeUser>>,
    pub(crate) tokens: Arc<TokenFile>,
    pub(crate) platform: Arc<RecordingPlatform>,
    pub(crate) bridge: WebBridge,
    pub(crate) clock: Arc<ManualClock>,
    pub(crate) activity: Arc<Activity>,
    /// An async lock: `start` and `stop` hold it while the core starts or
    /// quits, so no other control call sees it half way.
    pub(crate) core: tokio::sync::Mutex<Core>,
    /// The budget for the next start.
    pub(crate) quota: Mutex<QuotaConfig>,
    pub(crate) oauth: OAuthConfig,
    pub(crate) data_dir: PathBuf,
    pub(crate) dist: PathBuf,
    pub(crate) hits: Mutex<Vec<Hit>>,
}

/// A budget that never makes a request wait, so big seeds sync in seconds.
pub(crate) fn fast_quota() -> QuotaConfig {
    QuotaConfig {
        capacity: sm_gmail::Units(10_000_000),
        refill_per_minute: sm_gmail::Units(100_000_000),
        background_per_minute: sm_gmail::Units(100_000_000),
        background_reserve: sm_gmail::Units(0),
        ..QuotaConfig::default()
    }
}

impl Server {
    /// The core's configuration with `quota`, against the fake.
    fn config(&self, quota: QuotaConfig) -> Config {
        Config::in_data_dir(
            &self.data_dir,
            cfg!(debug_assertions),
            GmailConfig {
                base_url: self.fake.base_url(),
                oauth: self.oauth.clone(),
                quota,
            },
        )
    }

    /// Starts a core on the server's database, not launched yet.
    pub(crate) fn open_core(&self, quota: QuotaConfig) -> Result<Arc<App>, sm_core::Error> {
        let deps = Deps {
            platform: Arc::<RecordingPlatform>::clone(&self.platform),
            clock: Arc::<ManualClock>::clone(&self.clock),
            tokens: Arc::<TokenFile>::clone(&self.tokens),
            activity: Arc::clone(&self.activity),
        };
        Ok(Arc::new(App::start(&self.config(quota), deps)?))
    }

    /// The database file.
    pub(crate) fn db_path(&self) -> PathBuf {
        self.data_dir.join(Config::DB_FILE)
    }

    /// The launched core, if it is up.
    pub(crate) async fn running(&self) -> Option<Arc<App>> {
        match &*self.core.lock().await {
            Core::Running(app) => Some(Arc::clone(app)),
            Core::Ready { .. } | Core::Down => None,
        }
    }

    /// The core, launched or still seeding, if it is up.
    pub(crate) async fn open(&self) -> Option<Arc<App>> {
        match &*self.core.lock().await {
            Core::Running(app) | Core::Ready { app, .. } => Some(Arc::clone(app)),
            Core::Down => None,
        }
    }

    /// Polls every account now, if the core is up.
    pub(crate) async fn poll(&self) {
        if let Some(app) = self.running().await {
            app.accounts().poll_now();
        }
    }

    /// Retries every account's backed-off work and polls, as a network
    /// change would, if the core is up.
    pub(crate) async fn retry(&self) {
        if let Some(app) = self.running().await {
            app.accounts().retry_now();
            app.accounts().poll_now();
        }
    }

    /// Gmail's id for a local thread, if the core is up and has it.
    pub(crate) async fn gmail_thread(&self, thread: ThreadId) -> Option<GmailThreadId> {
        let app = self.open().await?;
        match app.queries().gmail_thread(thread).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(error = %e, "couldn't read a thread's Gmail id");
                None
            }
        }
    }

    /// The fake user for `email`.
    pub(crate) fn user(&self, email: &str) -> Result<FakeUser, Fail> {
        self.users
            .lock()
            .get(&email.to_lowercase())
            .cloned()
            .ok_or_else(|| Fail(format!("no fake user {email}")))
    }

    /// App time.
    pub(crate) fn now(&self) -> Millis {
        self.clock.now()
    }

    /// Whether the core has nothing left to do. Requests parked by a hold are
    /// waiting on the test, not on the core. While one is parked, a kick
    /// whose task wasn't waiting may belong to the task parked behind it, so
    /// it counts as waiting on the test too.
    fn is_idle(&self) -> bool {
        let busy = self.activity.snapshot();
        let held = self.fake.held();
        let deferred = busy.get(DEFERRED_KICK).copied().unwrap_or(0);
        let total: usize = busy.values().sum();
        let behind_holds = if held > 0 { deferred } else { 0 };
        total.saturating_sub(behind_holds) <= held
    }

    /// Waits until the core is idle, or fails with what is still busy.
    pub(crate) async fn settle(&self) -> Result<(), Fail> {
        self.settle_within(SETTLE_LIMIT).await
    }

    /// [`Server::settle`] with its own limit.
    pub(crate) async fn settle_within(&self, limit: Duration) -> Result<(), Fail> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let changed = self.activity.changed();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.is_idle() {
                return Ok(());
            }
            tokio::select! {
                () = changed => {}
                // Holds and Gmail's answers change without core activity.
                () = tokio::time::sleep(Duration::from_millis(5)) => {}
                () = tokio::time::sleep_until(deadline) => {
                    return Err(Fail(format!(
                        "the core did not settle in {}s; busy: {:?}, held: {}",
                        limit.as_secs(),
                        self.activity.snapshot(),
                        self.fake.held(),
                    )));
                }
            }
        }
    }
}

/// A failed control call: the test sees the message.
#[derive(Debug)]
pub(crate) struct Fail(pub(crate) String);

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": self.0 }))).into_response()
    }
}

impl<E: std::error::Error + 'static> From<E> for Fail {
    fn from(e: E) -> Self {
        Self(sm_bridge::commands::message(&e))
    }
}

/// A control call's answer.
pub(crate) type Ctl<T = Value> = Result<Json<T>, Fail>;

/// The answer of a control call that returns nothing.
pub(crate) const fn done() -> Json<Value> {
    Json(Value::Null)
}
