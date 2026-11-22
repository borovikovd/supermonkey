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
use sm_bridge::{Session, WebBridge};
use sm_core::activity::{Activity, DEFERRED_KICK};
use sm_core::clock::{Clock as _, ManualClock};
use sm_core::{App, Config, Deps, GmailConfig};
use sm_domain::{GmailThreadId, Millis, ThreadId};
use sm_fake_gmail::{FakeGmail, FakeUser};
use sm_gmail::{OAuthConfig, QuotaConfig, TokenFile};
use tokio::sync::Notify;

use crate::platform::RecordingPlatform;

pub(crate) const SETTLE_LIMIT: Duration = Duration::from_secs(10);

pub(crate) enum Core {
    Ready {
        app: Arc<App>,
        quota: QuotaConfig,
    },
    Running {
        session: Arc<Session>,
        exited: Arc<Notify>,
    },
    Down,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Hit {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) body: String,
    pub(crate) at: Millis,
}

pub(crate) struct Server {
    pub(crate) fake: FakeGmail,
    pub(crate) users: Mutex<HashMap<String, FakeUser>>,
    pub(crate) tokens: Arc<TokenFile>,
    pub(crate) platform: Arc<RecordingPlatform>,
    pub(crate) bridge: WebBridge,
    pub(crate) clock: Arc<ManualClock>,
    pub(crate) activity: Arc<Activity>,
    #[expect(
        clippy::disallowed_types,
        reason = "held while the core starts or quits, which awaits"
    )]
    pub(crate) core: tokio::sync::Mutex<Core>,
    pub(crate) quota: Mutex<QuotaConfig>,
    pub(crate) oauth: OAuthConfig,
    pub(crate) data_dir: PathBuf,
    pub(crate) dist: PathBuf,
    pub(crate) hits: Mutex<Vec<Hit>>,
}

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

    pub(crate) fn open_core(&self, quota: QuotaConfig) -> Result<Arc<App>, sm_core::Error> {
        let deps = Deps {
            platform: Arc::<RecordingPlatform>::clone(&self.platform),
            clock: Arc::<ManualClock>::clone(&self.clock),
            tokens: Arc::<TokenFile>::clone(&self.tokens),
            activity: Arc::clone(&self.activity),
        };
        Ok(Arc::new(App::start(self.config(quota), deps)?))
    }

    pub(crate) fn db_path(&self) -> PathBuf {
        self.data_dir.join(Config::DB_FILE)
    }

    pub(crate) async fn session(&self) -> Option<Arc<Session>> {
        match &*self.core.lock().await {
            Core::Running { session, .. } => Some(Arc::clone(session)),
            Core::Ready { .. } | Core::Down => None,
        }
    }

    pub(crate) async fn running(&self) -> Option<Arc<App>> {
        Some(Arc::clone(self.session().await?.core()))
    }

    pub(crate) async fn open(&self) -> Option<Arc<App>> {
        match &*self.core.lock().await {
            Core::Running { session, .. } => Some(Arc::clone(session.core())),
            Core::Ready { app, .. } => Some(Arc::clone(app)),
            Core::Down => None,
        }
    }

    pub(crate) async fn poll(&self) {
        if let Some(app) = self.running().await {
            app.accounts().poll_now();
        }
    }

    pub(crate) async fn retry(&self) {
        if let Some(app) = self.running().await {
            app.accounts().retry_now();
            app.accounts().poll_now();
        }
    }

    pub(crate) async fn gmail_thread(&self, thread: ThreadId) -> Option<GmailThreadId> {
        let app = self.open().await?;
        app.queries()
            .gmail_thread(thread)
            .await
            .unwrap_or_else(|e| unread_gmail_id(&e))
    }

    pub(crate) fn user(&self, email: &str) -> Result<FakeUser, Fail> {
        self.users
            .lock()
            .get(&email.to_lowercase())
            .cloned()
            .ok_or_else(|| Fail(format!("no fake user {email}")))
    }

    pub(crate) fn now(&self) -> Millis {
        self.clock.now()
    }

    fn is_idle(&self) -> bool {
        let busy = self.activity.snapshot();
        let held = self.fake.held();
        let deferred = busy.get(DEFERRED_KICK).copied().unwrap_or(0);
        let total: usize = busy.values().sum();
        let behind_holds = if held > 0 { deferred } else { 0 };
        total.saturating_sub(behind_holds) <= held
    }

    pub(crate) async fn settle(&self) -> Result<(), Fail> {
        self.settle_within(SETTLE_LIMIT).await
    }

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

fn unread_gmail_id(e: &sm_core::Error) -> Option<GmailThreadId> {
    tracing::warn!(error = %e, "couldn't read a thread's Gmail id");
    None
}

#[derive(Debug)]
pub(crate) struct Fail(pub(crate) String);

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": self.0 }))).into_response()
    }
}

impl<E: std::error::Error + 'static> From<E> for Fail {
    fn from(e: E) -> Self {
        Self(sm_bridge::commands::chain(&e))
    }
}

pub(crate) type Ctl<T = Value> = Result<Json<T>, Fail>;

pub(crate) const fn done() -> Json<Value> {
    Json(Value::Null)
}
