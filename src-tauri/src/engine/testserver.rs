//! The E2E server: the real core and the web bridge against a fake Gmail,
//! plus the built UI and a `/__test` control API, all on one origin.
//!
//! Lifecycle: the process starts the fake Gmail, an empty database, and a
//! manual app clock, then waits. A test seeds accounts and mail through
//! `/__test`, calls `POST /__test/start` to launch the engine, and only then
//! loads the page. `POST /__test/stop` quits (or crashes) the core in-process;
//! the next `start` relaunches it on the same database, fake Gmail, and clock.
//!
//! Time moves only through `/__test/clock`. `/__test/idle` waits until the core
//! has nothing left to do (see [`crate::clock`]).

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use base64::Engine as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ub_fake_gmail::{
    AddedDraft, AddedMessage, Answer, FakeGmail, FakeUser, Fault, LoggedRequest, Matcher,
    NewAttachment, NewMessage, Release,
};
use ub_gmail::{OAuthConfig, QuotaConfig, RefreshToken, TokenFile, TokenStore};

use super::webbridge::Bridge;
use super::{Config, Engine, Platform};
use crate::clock::Manual;
use crate::db::ReadPool;
use crate::model::{AccountId, ThreadId};
use crate::notify::Notification;
use crate::writer::{EffectReceiver, Writer};

/// How long `advance`, `release`, and a graceful stop wait for the core;
/// `idle` takes its own limit.
const SETTLE_LIMIT: Duration = Duration::from_secs(10);

/// What the app asked of the OS, for tests to assert on.
#[derive(Default)]
struct Recorded {
    notifications: Vec<Notification>,
    badge: Option<i64>,
    opened_urls: Vec<String>,
    opened_files: Vec<String>,
    focus_requests: usize,
}

/// How the next Google sign-in in the browser goes. Used once.
#[derive(Default)]
enum SignIn {
    /// The fake signs in its first user.
    #[default]
    FirstUser,
    As(String),
    /// The person closes the browser tab: nothing comes back.
    Abandon,
}

/// Records every call. Opening the sign-in page "completes" it with a plain
/// HTTP request: the fake consent page answers at once and redirects to the
/// app's loopback listener.
struct TestPlatform {
    recorded: Mutex<Recorded>,
    sign_in: Mutex<SignIn>,
    http: reqwest::Client,
    /// The fake's consent page; other URLs are links, only recorded.
    auth_url: String,
}

impl Platform for TestPlatform {
    fn open_url(&self, url: &str) {
        self.recorded.lock().opened_urls.push(url.to_owned());
        if !url.starts_with(&self.auth_url) {
            return;
        }
        let mode = std::mem::take(&mut *self.sign_in.lock());
        let url = match mode {
            SignIn::Abandon => {
                crate::clock::waiting_on_person();
                return;
            }
            SignIn::FirstUser => url.to_owned(),
            SignIn::As(email) => match reqwest::Url::parse(url) {
                Ok(mut u) => {
                    u.query_pairs_mut().append_pair("login_hint", &email);
                    u.to_string()
                }
                Err(e) => {
                    log::warn!("test sign-in: {e}");
                    return;
                }
            },
        };
        let http = self.http.clone();
        crate::clock::spawn("browser", async move {
            if let Err(e) = http.get(&url).send().await {
                log::warn!("test sign-in: {e}");
            }
        });
    }

    fn open_path(&self, path: &FsPath) -> anyhow::Result<()> {
        self.recorded
            .lock()
            .opened_files
            .push(path.to_string_lossy().into_owned());
        Ok(())
    }

    fn notify(&self, n: &Notification) {
        self.recorded.lock().notifications.push(n.clone());
    }

    fn set_badge(&self, count: Option<i64>) {
        self.recorded.lock().badge = count;
    }

    fn focus_window(&self) {
        self.recorded.lock().focus_requests += 1;
    }
}

/// The app process, as the test sees it.
enum Core {
    /// Seeding: the database is open; the engine has not started.
    Ready {
        writer: Writer,
        reads: ReadPool,
        effects: EffectReceiver,
    },
    Running(Arc<Engine>),
    /// Quit or crashed; `start` relaunches it.
    Down,
}

/// A request to the remote image host or the `evil.test` sink.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Hit {
    method: String,
    path: String,
    at: i64,
}

struct Server {
    fake: FakeGmail,
    users: Mutex<HashMap<String, FakeUser>>,
    store: Arc<TokenFile>,
    platform: Arc<TestPlatform>,
    bridge: Bridge,
    clock: &'static Manual,
    core: tokio::sync::Mutex<Core>,
    quota: Mutex<QuotaConfig>,
    oauth: OAuthConfig,
    data_dir: PathBuf,
    db: PathBuf,
    dist: PathBuf,
    hits: Mutex<Vec<Hit>>,
}

/// A budget that never makes a request wait, so big seeds sync in seconds.
fn fast_quota() -> QuotaConfig {
    QuotaConfig {
        capacity: 10_000_000,
        refill_per_minute: 100_000_000,
        background_per_minute: 100_000_000,
        background_reserve: 0,
        ..QuotaConfig::default()
    }
}

fn open_core(db: &FsPath) -> anyhow::Result<Core> {
    let (effects_tx, effects) = tokio::sync::mpsc::unbounded_channel();
    Ok(Core::Ready {
        writer: Writer::start(db, effects_tx)?,
        reads: ReadPool::new(db, 4),
        effects,
    })
}

/// Runs the server until the process is killed. Prints `READY <url>` once
/// listening. App time starts at `now_ms`.
pub async fn run(port: u16, dist: PathBuf, now_ms: i64) -> anyhow::Result<()> {
    let clock = Manual::install(now_ms)?;
    let dir = tempfile::tempdir()?;
    let data_dir = dir.path().to_path_buf();
    let _ = crate::drafts::DRAFT_FILES.set(data_dir.join("drafts"));
    let db = data_dir.join("e2e.db");
    let fake = FakeGmail::start();
    fake.set_clock(|| u64::try_from(crate::clock::now_ms()).unwrap_or(0));
    let oauth = OAuthConfig {
        client_id: "e2e".into(),
        client_secret: "e2e".into(),
        auth_url: fake.auth_url(),
        token_url: fake.token_url(),
        revoke_url: fake.revoke_url(),
    };
    let platform = Arc::new(TestPlatform {
        recorded: Mutex::new(Recorded::default()),
        sign_in: Mutex::new(SignIn::default()),
        http: reqwest::Client::new(),
        auth_url: fake.auth_url(),
    });
    let bridge = Bridge::new(None);
    let server = Arc::new(Server {
        core: tokio::sync::Mutex::new(open_core(&db)?),
        fake,
        users: Mutex::new(HashMap::new()),
        store: Arc::new(TokenFile::new(data_dir.join(super::TOKEN_FILE))),
        platform,
        bridge: bridge.clone(),
        clock,
        quota: Mutex::new(fast_quota()),
        oauth,
        data_dir,
        db,
        dist,
        hits: Mutex::new(Vec::new()),
    });
    let app = super::webbridge::router(bridge).merge(control(server));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let addr = listener.local_addr()?;
    #[expect(
        clippy::print_stdout,
        reason = "The test runner reads the address from stdout."
    )]
    {
        println!("READY http://{addr}");
    }
    axum::serve(listener, app).await?;
    drop(dir);
    Ok(())
}

fn control(server: Arc<Server>) -> Router {
    Router::new()
        .route("/__test/account", post(add_account))
        .route("/__test/user", post(add_user))
        .route("/__test/quota", post(quota))
        .route("/__test/start", post(start))
        .route("/__test/stop", post(stop))
        .route("/__test/idle", get(idle))
        .route("/__test/deliver", post(deliver))
        .route("/__test/deliver-batch", post(deliver_batch))
        .route("/__test/gmail/{email}", get(gmail_state))
        .route("/__test/gmail/{email}/modify", post(gmail_modify))
        .route("/__test/gmail/{email}/delete", post(gmail_delete))
        .route(
            "/__test/gmail/{email}/delete-thread",
            post(gmail_delete_thread),
        )
        .route("/__test/gmail/{email}/label", post(gmail_label))
        .route("/__test/gmail/{email}/send-as", post(gmail_send_as))
        .route("/__test/gmail/{email}/drafts", post(gmail_create_draft))
        .route(
            "/__test/gmail/{email}/drafts/{id}",
            post(gmail_update_draft),
        )
        .route(
            "/__test/gmail/{email}/drafts/{id}/delete",
            post(gmail_delete_draft),
        )
        .route(
            "/__test/gmail/{email}/drafts/{id}/send",
            post(gmail_send_draft),
        )
        .route(
            "/__test/gmail/{email}/expire-tokens",
            post(gmail_expire_tokens),
        )
        .route(
            "/__test/gmail/{email}/expire-history",
            post(gmail_expire_history),
        )
        .route("/__test/invalid-grant", post(invalid_grant))
        .route("/__test/sign-in", post(next_sign_in))
        .route("/__test/tokens", get(tokens))
        .route("/__test/tokens/mode", get(tokens_mode))
        .route("/__test/tokens/forget", post(forget_token))
        .route("/__test/revocations", get(revocations))
        .route("/__test/fault", post(fault))
        .route("/__test/release", post(release))
        .route("/__test/clear-faults", post(clear_faults))
        .route("/__test/offline", post(offline))
        .route("/__test/requests", get(requests))
        .route("/__test/poll", post(poll))
        .route("/__test/clock", get(clock_now).post(clock))
        .route("/__test/focus", post(focus))
        .route("/__test/platform", get(platform))
        .route("/__test/clear-recorded", post(clear_recorded))
        .route("/__test/hits", get(hits))
        .route("/__test/db-contains", get(db_contains))
        .route("/img/{name}", get(remote_image))
        .route("/sink/{*rest}", any(sink))
        .fallback(get(static_file))
        .with_state(server)
}

/// A failed control call: the test sees the message.
struct Fail(String);

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": self.0 }))).into_response()
    }
}

impl<E: std::fmt::Display> From<E> for Fail {
    fn from(e: E) -> Self {
        Fail(format!("{e:#}"))
    }
}

type Ctl<T = Value> = Result<Json<T>, Fail>;

/// The body of a control call that returns nothing.
fn done() -> Json<Value> {
    Json(Value::Null)
}

fn user(s: &Server, email: &str) -> Result<FakeUser, Fail> {
    s.users
        .lock()
        .get(&email.to_lowercase())
        .cloned()
        .ok_or_else(|| Fail(format!("no fake user {email}")))
}

impl Server {
    /// The running engine, if the core is up.
    async fn engine(&self) -> Option<Arc<Engine>> {
        match &*self.core.lock().await {
            Core::Running(e) => Some(Arc::clone(e)),
            Core::Ready { .. } | Core::Down => None,
        }
    }

    /// Polls every account now, if the core is up.
    async fn kick_polls(&self) {
        if let Some(e) = self.engine().await {
            e.kick_polls();
        }
    }

    /// Whether the core has nothing left to do. Requests parked by a hold are
    /// waiting on the test, not on the core. While one is parked, a kick whose
    /// task wasn't waiting may belong to the task parked behind it, so it
    /// counts as waiting on the test too.
    fn core_idle(&self) -> bool {
        let busy = crate::clock::activity();
        let held = self.fake.held();
        let deferred = busy.get(crate::clock::DEFERRED_KICK).copied().unwrap_or(0);
        let total: usize = busy.values().sum();
        let behind_holds = if held > 0 { deferred } else { 0 };
        total.saturating_sub(behind_holds) <= held
    }

    /// Waits until [`Server::core_idle`], or fails with what is still busy.
    async fn settle(&self) -> Result<(), Fail> {
        self.settle_within(SETTLE_LIMIT).await
    }

    async fn settle_within(&self, limit: Duration) -> Result<(), Fail> {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let changed = crate::clock::activity_changed();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.core_idle() {
                return Ok(());
            }
            tokio::select! {
                () = changed => {}
                // Holds and Gmail answers change without core activity.
                () = tokio::time::sleep(Duration::from_millis(5)) => {}
                () = tokio::time::sleep_until(deadline) => {
                    return Err(Fail(format!(
                        "the core did not settle in {}s; busy: {:?}, held: {}",
                        limit.as_secs(),
                        crate::clock::activity(),
                        self.fake.held(),
                    )));
                }
            }
        }
    }

    /// Gmail thread id of a local thread, if the core is up and knows it.
    async fn gmail_thread(&self, thread: ThreadId) -> Option<String> {
        let e = self.engine().await?;
        e.reads
            .with(|c| {
                Ok(c.query_row(
                    "SELECT gmail_id FROM threads WHERE id = ?1",
                    [thread],
                    |r| r.get::<_, Option<String>>(0),
                )?)
            })
            .ok()
            .flatten()
    }

    async fn account_email(&self, account: AccountId) -> Option<String> {
        let e = self.engine().await?;
        e.reads
            .with(|c| {
                Ok(
                    c.query_row("SELECT email FROM accounts WHERE id = ?1", [account], |r| {
                        r.get(0)
                    })?,
                )
            })
            .ok()
    }
}

// ------------------------------------------------------------ accounts

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserReq {
    email: String,
    #[serde(default)]
    name: Option<String>,
}

fn create_user(s: &Server, req: &UserReq) -> FakeUser {
    let u = s.fake.add_user(&req.email);
    if let Some(name) = &req.name {
        u.set_display_name(name);
    }
    s.users.lock().insert(req.email.to_lowercase(), u.clone());
    u
}

/// A Gmail user that exists in the fake but not in the app (for Add account).
async fn add_user(State(s): State<Arc<Server>>, Json(req): Json<UserReq>) -> Ctl {
    create_user(&s, &req);
    Ok(done())
}

/// A Gmail user already signed in to the app (before `/__test/start`).
async fn add_account(State(s): State<Arc<Server>>, Json(req): Json<UserReq>) -> Ctl {
    let writer = match &*s.core.lock().await {
        Core::Ready { writer, .. } => writer.clone(),
        Core::Running(e) => e.writer.clone(),
        Core::Down => return Err(Fail("the core is down; start it first".into())),
    };
    let u = create_user(&s, &req);
    s.store
        .store_refresh_token(&req.email, &RefreshToken(u.refresh_token().to_owned()))
        .map_err(|e| Fail(e.0))?;
    let email = req.email;
    let id = writer
        .call(move |ctx| crate::accounts::create(ctx, &email))
        .await?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
struct QuotaReq {
    production: bool,
}

/// The request budget for the next start: production's, or one that never waits.
async fn quota(State(s): State<Arc<Server>>, Json(req): Json<QuotaReq>) -> Ctl {
    *s.quota.lock() = if req.production {
        QuotaConfig::default()
    } else {
        fast_quota()
    };
    Ok(done())
}

// ------------------------------------------------------------ lifecycle

/// Launches the engine: the seeded accounts start syncing. After a stop, the
/// app relaunches on the same database.
async fn start(State(s): State<Arc<Server>>) -> Ctl {
    let mut core = s.core.lock().await;
    let (writer, reads, effects) = match std::mem::replace(&mut *core, Core::Down) {
        Core::Ready {
            writer,
            reads,
            effects,
        } => (writer, reads, effects),
        Core::Down => match open_core(&s.db)? {
            Core::Ready {
                writer,
                reads,
                effects,
            } => (writer, reads, effects),
            other => {
                *core = other;
                return Err(Fail("could not open the database".into()));
            }
        },
        running @ Core::Running(_) => {
            *core = running;
            return Err(Fail("already running".into()));
        }
    };
    let config = Config {
        gmail_base: s.fake.base_url(),
        oauth: s.oauth.clone(),
        quota: s.quota.lock().clone(),
    };
    let engine = Engine::new(
        Arc::<TestPlatform>::clone(&s.platform),
        writer,
        reads,
        s.data_dir.clone(),
        config,
        Arc::<TokenFile>::clone(&s.store),
    );
    s.bridge.set_engine(Some(Arc::clone(&engine)));
    engine.run(effects);
    *core = Core::Running(engine);
    Ok(done())
}

#[derive(Deserialize)]
struct StopReq {
    /// Quit as `Cmd+Q` does (the page closes an open composer first); otherwise crash.
    graceful: bool,
}

/// Quits or crashes the core. Fake Gmail and the clock live on; `start` relaunches.
async fn stop(State(s): State<Arc<Server>>, Json(req): Json<StopReq>) -> Ctl {
    let mut core = s.core.lock().await;
    let engine = match std::mem::replace(&mut *core, Core::Down) {
        Core::Running(e) => e,
        other => {
            *core = other;
            return Err(Fail("the core is not running".into()));
        }
    };
    if req.graceful {
        let quit = s.bridge.quit_signal().notified();
        tokio::pin!(quit);
        quit.as_mut().enable();
        if s.bridge.emit("quit-requested", Value::Null) > 0
            && tokio::time::timeout(SETTLE_LIMIT, quit).await.is_err()
        {
            log::warn!("the page did not call quit_now");
        }
        // The composer close the page sent reaches the database before the core stops.
        s.settle().await?;
    }
    s.bridge.set_engine(None);
    engine.shutdown().await;
    Ok(done())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdleReq {
    /// Default 10 s.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// Waits until the core is idle. Returns where the page's channel should be
/// and the sign-ins the core is running, for the page's half of `settle()`.
async fn idle(State(s): State<Arc<Server>>, Query(q): Query<IdleReq>) -> Ctl {
    s.settle_within(q.timeout_ms.map_or(SETTLE_LIMIT, Duration::from_millis))
        .await?;
    let (rev, ack) = s.bridge.last_sent();
    Ok(Json(json!({
        "rev": rev,
        "ack": ack,
        "signIns": s.bridge.sign_ins(),
        "held": s.fake.held(),
    })))
}

// ------------------------------------------------------------ mail

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentReq {
    filename: String,
    #[serde(default)]
    mime_type: Option<String>,
    /// Plain text content, or base64 with `base64: true`.
    #[serde(default)]
    content: String,
    #[serde(default)]
    base64: bool,
    #[serde(default)]
    content_id: Option<String>,
}

fn attachments(list: Vec<AttachmentReq>) -> Result<Vec<NewAttachment>, Fail> {
    list.into_iter()
        .map(|a| {
            let data = if a.base64 {
                base64::engine::general_purpose::STANDARD.decode(a.content)?
            } else {
                a.content.into_bytes()
            };
            Ok(NewAttachment {
                filename: a.filename,
                mime_type: a
                    .mime_type
                    .unwrap_or_else(|| "application/octet-stream".into()),
                data,
                content_id: a.content_id,
            })
        })
        .collect()
}

fn app_now() -> u64 {
    u64::try_from(crate::clock::now_ms()).unwrap_or(0)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeliverReq {
    /// The mailbox the message lands in.
    account: String,
    from: String,
    #[serde(default)]
    to: Vec<String>,
    #[serde(default)]
    cc: Vec<String>,
    subject: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    html: Option<String>,
    /// Defaults to `INBOX`, `UNREAD`.
    #[serde(default)]
    labels: Option<Vec<String>>,
    /// Joins this Gmail thread.
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    in_reply_to: Option<String>,
    #[serde(default)]
    references: Option<String>,
    #[serde(default)]
    list_id: Option<String>,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    attachments: Vec<AttachmentReq>,
    /// Unix ms; defaults to app time.
    #[serde(default)]
    date: Option<u64>,
    /// In a batch: joins the thread of this earlier item and replies to it.
    #[serde(default)]
    after: Option<usize>,
    /// Poll at once, so the app sees the message without waiting.
    #[serde(default = "yes")]
    poll: bool,
}

fn yes() -> bool {
    true
}

/// Puts a message in a mailbox, as if it arrived or was filed in Gmail web.
async fn deliver(State(s): State<Arc<Server>>, Json(req): Json<DeliverReq>) -> Ctl {
    let poll = req.poll;
    let added = deliver_one(&s, req)?;
    if poll {
        s.kick_polls().await;
    }
    Ok(Json(added_json(&added)))
}

fn added_json(m: &AddedMessage) -> Value {
    json!({ "id": m.id, "threadId": m.thread_id, "messageId": m.rfc822_message_id })
}

/// Many deliveries in one call, in order (whole seeds). An item's `after`
/// names an earlier item whose thread it joins. Polls once at the end if any asks.
async fn deliver_batch(State(s): State<Arc<Server>>, Json(list): Json<Vec<DeliverReq>>) -> Ctl {
    let poll = list.iter().any(|d| d.poll);
    let mut added: Vec<AddedMessage> = Vec::with_capacity(list.len());
    for mut req in list {
        if let Some(i) = req.after {
            let prev = added
                .get(i)
                .ok_or_else(|| Fail(format!("`after: {i}` names no earlier item")))?;
            req.thread_id = Some(prev.thread_id.clone());
            req.in_reply_to
                .get_or_insert_with(|| prev.rfc822_message_id.clone());
        }
        added.push(deliver_one(&s, req)?);
    }
    if poll {
        s.kick_polls().await;
    }
    Ok(Json(json!(
        added.iter().map(added_json).collect::<Vec<_>>()
    )))
}

fn deliver_one(s: &Server, req: DeliverReq) -> Result<AddedMessage, Fail> {
    let u = user(s, &req.account)?;
    let added = u.add_message(&NewMessage {
        from: req.from,
        to: if req.to.is_empty() {
            vec![u.email().to_owned()]
        } else {
            req.to
        },
        cc: req.cc,
        subject: req.subject,
        text: req.text,
        html: req.html,
        labels: req
            .labels
            .unwrap_or_else(|| vec!["INBOX".into(), "UNREAD".into()]),
        thread_id: req.thread_id,
        in_reply_to: req.in_reply_to,
        references: req.references,
        list_id: req.list_id,
        extra_headers: req.headers,
        attachments: attachments(req.attachments)?,
        internal_date: Some(req.date.unwrap_or_else(app_now)),
        ..Default::default()
    });
    Ok(added)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageView {
    id: String,
    thread_id: String,
    labels: Vec<String>,
    date: u64,
    snippet: String,
    headers: HashMap<String, String>,
    /// The raw RFC 822 message, for checking what was sent.
    raw: String,
}

fn message_view(m: ub_fake_gmail::FakeMessage) -> MessageView {
    MessageView {
        id: m.id,
        thread_id: m.thread_id,
        labels: m.label_ids,
        date: m.internal_date,
        snippet: m.snippet,
        headers: m
            .headers
            .into_iter()
            .map(|(k, v)| (k.to_lowercase(), v))
            .collect(),
        raw: String::from_utf8(m.raw)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }
}

/// The mailbox as Gmail sees it: messages, drafts, labels, and sendAs.
async fn gmail_state(State(s): State<Arc<Server>>, Path(email): Path<String>) -> Ctl {
    let u = user(&s, &email)?;
    let messages: Vec<MessageView> = u.messages().into_iter().map(message_view).collect();
    let drafts: Vec<Value> = u
        .drafts()
        .into_iter()
        .map(|d| json!({ "id": d.id, "message": message_view(d.message) }))
        .collect();
    let labels: Vec<Value> = u
        .labels()
        .into_iter()
        .map(|(id, name)| json!({ "id": id, "name": name }))
        .collect();
    let send_as: Vec<Value> = u
        .send_as()
        .into_iter()
        .map(|(email, name, primary)| {
            json!({ "email": email, "displayName": name, "primary": primary })
        })
        .collect();
    Ok(Json(json!({
        "messages": messages,
        "drafts": drafts,
        "labels": labels,
        "sendAs": send_as,
        "historyId": u.history_id(),
    })))
}

/// Gmail-side changes poll only when asked: tests usually poll as a step.
#[derive(Deserialize, Default)]
struct PollOpt {
    #[serde(default)]
    poll: bool,
}

#[derive(Deserialize)]
struct ModifyReq {
    id: String,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(default)]
    poll: bool,
}

/// A label change made outside the app (Gmail web, a filter).
async fn gmail_modify(
    State(s): State<Arc<Server>>,
    Path(email): Path<String>,
    Json(req): Json<ModifyReq>,
) -> Ctl {
    let u = user(&s, &email)?;
    let add: Vec<&str> = req.add.iter().map(String::as_str).collect();
    let remove: Vec<&str> = req.remove.iter().map(String::as_str).collect();
    if u.message(&req.id).is_none() {
        return Err(Fail(format!("no message {}", req.id)));
    }
    u.modify_labels(&req.id, &add, &remove);
    if req.poll {
        s.kick_polls().await;
    }
    Ok(done())
}

#[derive(Deserialize)]
struct IdReq {
    id: String,
    #[serde(default)]
    poll: bool,
}

/// A message deleted outside the app.
async fn gmail_delete(
    State(s): State<Arc<Server>>,
    Path(email): Path<String>,
    Json(req): Json<IdReq>,
) -> Ctl {
    let u = user(&s, &email)?;
    if u.message(&req.id).is_none() {
        return Err(Fail(format!("no message {}", req.id)));
    }
    u.delete_message(&req.id);
    if req.poll {
        s.kick_polls().await;
    }
    Ok(done())
}

/// A whole thread deleted outside the app.
async fn gmail_delete_thread(
    State(s): State<Arc<Server>>,
    Path(email): Path<String>,
    Json(req): Json<IdReq>,
) -> Ctl {
    user(&s, &email)?.delete_thread(&req.id);
    if req.poll {
        s.kick_polls().await;
    }
    Ok(done())
}

#[derive(Deserialize)]
struct NameReq {
    name: String,
}

/// A label created outside the app. Returns its Gmail id.
async fn gmail_label(
    State(s): State<Arc<Server>>,
    Path(email): Path<String>,
    Json(req): Json<NameReq>,
) -> Ctl {
    let u = user(&s, &email)?;
    if u.labels()
        .iter()
        .any(|(_, n)| n.eq_ignore_ascii_case(&req.name))
    {
        return Err(Fail(format!("label {} exists", req.name)));
    }
    Ok(Json(json!({ "id": u.add_label(&req.name) })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendAsReq {
    email: String,
    display_name: String,
}

async fn gmail_send_as(
    State(s): State<Arc<Server>>,
    Path(email): Path<String>,
    Json(req): Json<SendAsReq>,
) -> Ctl {
    user(&s, &email)?.add_send_as(&req.email, &req.display_name);
    Ok(done())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DraftReq {
    /// Defaults to the mailbox's own address.
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Vec<String>,
    #[serde(default)]
    cc: Vec<String>,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    in_reply_to: Option<String>,
    #[serde(default)]
    references: Option<String>,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    attachments: Vec<AttachmentReq>,
    #[serde(default)]
    date: Option<u64>,
    #[serde(default)]
    poll: bool,
}

fn draft_message(u: &FakeUser, req: DraftReq) -> Result<NewMessage, Fail> {
    Ok(NewMessage {
        from: req.from.unwrap_or_else(|| u.email().to_owned()),
        to: req.to,
        cc: req.cc,
        subject: req.subject,
        text: req.text,
        html: req.html,
        thread_id: req.thread_id,
        references: req.references.or_else(|| req.in_reply_to.clone()),
        in_reply_to: req.in_reply_to,
        extra_headers: req.headers,
        attachments: attachments(req.attachments)?,
        internal_date: Some(req.date.unwrap_or_else(app_now)),
        ..Default::default()
    })
}

fn draft_json(d: &AddedDraft) -> Json<Value> {
    Json(json!({
        "id": d.id,
        "messageId": d.message_id,
        "threadId": d.thread_id,
        "rfc822MessageId": d.rfc822_message_id,
    }))
}

/// A draft made in another client (Gmail web, a phone).
async fn gmail_create_draft(
    State(s): State<Arc<Server>>,
    Path(email): Path<String>,
    Json(req): Json<DraftReq>,
) -> Ctl {
    let u = user(&s, &email)?;
    let poll = req.poll;
    let added = u.add_draft(&draft_message(&u, req)?);
    if poll {
        s.kick_polls().await;
    }
    Ok(draft_json(&added))
}

/// A draft edited in another client.
async fn gmail_update_draft(
    State(s): State<Arc<Server>>,
    Path((email, id)): Path<(String, String)>,
    Json(req): Json<DraftReq>,
) -> Ctl {
    let u = user(&s, &email)?;
    let poll = req.poll;
    let updated = u
        .update_draft(&id, &draft_message(&u, req)?)
        .ok_or_else(|| Fail(format!("no draft {id}")))?;
    if poll {
        s.kick_polls().await;
    }
    Ok(draft_json(&updated))
}

/// A draft deleted in another client.
async fn gmail_delete_draft(
    State(s): State<Arc<Server>>,
    Path((email, id)): Path<(String, String)>,
    body: Option<Json<PollOpt>>,
) -> Ctl {
    if !user(&s, &email)?.delete_draft(&id) {
        return Err(Fail(format!("no draft {id}")));
    }
    if body.is_some_and(|b| b.poll) {
        s.kick_polls().await;
    }
    Ok(done())
}

/// A draft sent from another client (a phone).
async fn gmail_send_draft(
    State(s): State<Arc<Server>>,
    Path((email, id)): Path<(String, String)>,
    body: Option<Json<PollOpt>>,
) -> Ctl {
    let sent = user(&s, &email)?
        .send_draft(&id)
        .ok_or_else(|| Fail(format!("no draft {id}")))?;
    if body.is_some_and(|b| b.poll) {
        s.kick_polls().await;
    }
    Ok(Json(json!({
        "id": sent.id,
        "threadId": sent.thread_id,
        "messageId": sent.rfc822_message_id,
    })))
}

/// The account's access tokens expire: its next Gmail call gets a 401.
async fn gmail_expire_tokens(State(s): State<Arc<Server>>, Path(email): Path<String>) -> Ctl {
    user(&s, &email)?.expire_access_tokens();
    Ok(done())
}

/// Gmail forgets the account's history: the next `history.list` answers 404.
async fn gmail_expire_history(State(s): State<Arc<Server>>, Path(email): Path<String>) -> Ctl {
    user(&s, &email)?.expire_history();
    Ok(done())
}

// ------------------------------------------------------------ OAuth

#[derive(Deserialize)]
struct InvalidGrantReq {
    email: String,
    on: bool,
}

/// Google revokes (or restores) the account's refresh token.
async fn invalid_grant(State(s): State<Arc<Server>>, Json(req): Json<InvalidGrantReq>) -> Ctl {
    let u = user(&s, &req.email)?;
    u.set_invalid_grant(req.on);
    if req.on {
        u.expire_access_tokens();
    }
    Ok(done())
}

#[derive(Deserialize)]
struct SignInReq {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    abandon: bool,
    #[serde(default)]
    deny: bool,
}

/// How the next sign-in in the browser goes: as a user, abandoned, or denied.
async fn next_sign_in(State(s): State<Arc<Server>>, Json(req): Json<SignInReq>) -> Ctl {
    if req.deny {
        s.fake.deny_next_sign_in();
    }
    *s.platform.sign_in.lock() = match (req.abandon, req.email) {
        (true, _) => SignIn::Abandon,
        (false, Some(email)) => SignIn::As(email),
        (false, None) => SignIn::FirstUser,
    };
    Ok(done())
}

/// The refresh-token file's entries, as stored on disk.
async fn tokens(State(s): State<Arc<Server>>) -> Ctl {
    let items: Vec<Value> = s
        .store
        .entries()
        .map_err(|e| Fail(e.0))?
        .into_iter()
        .map(|(account, token)| json!({ "account": account, "token": token.0 }))
        .collect();
    Ok(Json(json!(items)))
}

/// The refresh-token file's permission bits, such as "600", or null before the first save.
async fn tokens_mode(State(s): State<Arc<Server>>) -> Ctl {
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(s.data_dir.join(super::TOKEN_FILE))
        .ok()
        .map(|m| format!("{:o}", m.permissions().mode() & 0o777));
    Ok(Json(json!(mode)))
}

#[derive(Deserialize)]
struct ForgetTokenReq {
    email: String,
}

/// Deletes an account's refresh token from the file, as if it was never saved.
async fn forget_token(State(s): State<Arc<Server>>, Json(req): Json<ForgetTokenReq>) -> Ctl {
    s.store
        .delete_refresh_token(&req.email)
        .map_err(|e| Fail(e.0))?;
    Ok(done())
}

/// Tokens revoked at Google, oldest first.
async fn revocations(State(s): State<Arc<Server>>) -> Ctl {
    let list: Vec<Value> = s
        .fake
        .revocations()
        .into_iter()
        .map(|r| json!({ "token": r.token, "account": r.email }))
        .collect();
    Ok(Json(json!(list)))
}

// ------------------------------------------------------------ faults

/// Picks requests: by Gmail method (`api`, e.g. `messages.batchModify`), HTTP
/// method and path glob, account, and query parameters.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct MatchReq {
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    query: HashMap<String, String>,
    #[serde(default)]
    nth: Option<usize>,
    #[serde(default)]
    times: Option<usize>,
}

impl MatchReq {
    fn matcher(&self) -> Matcher {
        let mut m = Matcher::new(
            self.method.as_deref().unwrap_or("*"),
            self.path.as_deref().unwrap_or("*"),
        );
        if let Some(api) = &self.api {
            m = m.for_api(api);
        }
        if let Some(account) = &self.account {
            m = m.account(account);
        }
        for (k, v) in &self.query {
            m = m.query(k, v);
        }
        if let Some(n) = self.nth {
            m = m.nth(n);
        }
        if let Some(t) = self.times {
            m = m.times(t);
        }
        m
    }
}

/// What a matched request suffers.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FaultSpec {
    /// `drop`, `noResponse`, `status`, `statusAfterApply`, `delay`, `hold`, or `omitThreads`.
    kind: String,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    retry_after_secs: Option<u64>,
    #[serde(default)]
    delay_ms: Option<u64>,
    #[serde(default)]
    threads: Vec<String>,
}

impl FaultSpec {
    fn fault(self) -> Result<Fault, Fail> {
        let status = |s: Option<u16>| s.ok_or_else(|| Fail("a status fault needs `status`".into()));
        let reason = |r: Option<String>, status: u16| {
            r.unwrap_or_else(|| {
                match status {
                    429 => "rateLimitExceeded",
                    500..=599 => "backendError",
                    _ => "failedPrecondition",
                }
                .into()
            })
        };
        Ok(match self.kind.as_str() {
            "drop" => Fault::Drop,
            "noResponse" => Fault::NoResponse,
            "status" => {
                let code = status(self.status)?;
                Fault::Status {
                    status: code,
                    reason: reason(self.reason, code),
                    retry_after_secs: self.retry_after_secs,
                }
            }
            "statusAfterApply" => {
                let code = status(self.status)?;
                Fault::StatusAfterApply {
                    status: code,
                    reason: reason(self.reason, code),
                }
            }
            "delay" => Fault::Delay(Duration::from_millis(self.delay_ms.unwrap_or(1000))),
            "hold" => Fault::Hold,
            "holdAnswer" => Fault::HoldAnswer,
            "omitThreads" => Fault::OmitThreads(self.threads),
            other => return Err(Fail(format!("unknown fault kind {other}"))),
        })
    }
}

#[derive(Deserialize)]
struct FaultReq {
    #[serde(flatten)]
    on: MatchReq,
    #[serde(flatten)]
    fault: FaultSpec,
}

/// Makes matching Gmail requests fail or wait (SPEC Gmail errors). Returns the
/// fault's id, which `release` takes for a hold.
async fn fault(State(s): State<Arc<Server>>, Json(req): Json<FaultReq>) -> Ctl {
    let id = s.fake.inject(req.on.matcher(), req.fault.fault()?);
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseReq {
    /// One hold, by the id `fault` returned.
    #[serde(default)]
    id: Option<u64>,
    /// Holds on this Gmail method. With neither, every hold.
    #[serde(default)]
    api: Option<String>,
    /// What released requests suffer instead of being handled normally.
    #[serde(default)]
    outcome: Option<FaultSpec>,
    /// Waits until the holds have parked at least this many requests first, so
    /// a release never overtakes the request it is meant for.
    #[serde(default)]
    min: usize,
}

/// Ends holds and lets the requests they parked go on.
async fn release(State(s): State<Arc<Server>>, Json(req): Json<ReleaseReq>) -> Ctl {
    let which = match (req.id, req.api) {
        (Some(id), _) => Release::Hold(id),
        (None, Some(api)) => Release::Api(api),
        (None, None) => Release::All,
    };
    let outcome = req.outcome.map(FaultSpec::fault).transpose()?;
    let deadline = tokio::time::Instant::now() + SETTLE_LIMIT;
    while s.fake.held_by(&which) < req.min {
        if tokio::time::Instant::now() > deadline {
            return Err(Fail(format!(
                "release: {} of {} requests held after {}s",
                s.fake.held_by(&which),
                req.min,
                SETTLE_LIMIT.as_secs(),
            )));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let released = s.fake.release(&which, outcome.as_ref());
    Ok(Json(json!({ "released": released })))
}

/// Removes every fault and hold, then retries what is backing off, as a network
/// change would. Requests already held stay held until released.
async fn clear_faults(State(s): State<Arc<Server>>) -> Ctl {
    s.fake.clear_faults();
    if let Some(e) = s.engine().await {
        e.kick_queues();
        e.kick_polls();
    }
    Ok(done())
}

#[derive(Deserialize)]
struct OnReq {
    on: bool,
}

/// The network goes away (every Gmail request fails to connect) or comes back.
async fn offline(State(s): State<Arc<Server>>, Json(req): Json<OnReq>) -> Ctl {
    s.fake.set_offline(req.on).await;
    if !req.on
        && let Some(e) = s.engine().await
    {
        e.kick_polls();
        e.kick_queues();
    }
    Ok(done())
}

fn answer_json(a: Option<&Answer>) -> Value {
    match a {
        None => Value::Null,
        Some(Answer::Status(code)) => json!(code),
        Some(Answer::Dropped) => json!("dropped"),
        Some(Answer::NoResponse) => json!("noResponse"),
    }
}

fn request_json(r: &LoggedRequest) -> Value {
    let mut params = serde_json::Map::new();
    for (k, v) in r.info.query_pairs() {
        match params.get_mut(&k) {
            Some(Value::String(have)) => {
                have.push(',');
                have.push_str(&v);
            }
            _ => {
                params.insert(k, Value::String(v));
            }
        }
    }
    json!({
        "seq": r.seq,
        "api": r.info.api,
        "method": r.info.method,
        "path": r.info.path,
        "query": r.info.query,
        "params": params,
        "account": r.info.account,
        "body": r.body,
        "at": r.at_ms,
        "held": r.held,
        "answer": answer_json(r.answer.as_ref()),
        "origin": r.origin,
        "userAgent": r.user_agent,
    })
}

/// Every request the fake received, in arrival order.
async fn requests(State(s): State<Arc<Server>>) -> Ctl {
    let list: Vec<Value> = s.fake.requests().iter().map(request_json).collect();
    Ok(Json(json!(list)))
}

async fn poll(State(s): State<Arc<Server>>) -> Ctl {
    s.kick_polls().await;
    Ok(done())
}

// ------------------------------------------------------------ time

async fn clock_now() -> Ctl {
    Ok(Json(json!({ "now": crate::clock::now_ms() })))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ClockReq {
    /// Moves app time forward, firing each timer due by then once.
    #[serde(default)]
    advance_ms: Option<u64>,
    /// With `advance_ms`: fires timers one by one at their own times, letting
    /// the core settle between, so a back-off runs every retry it would.
    #[serde(default)]
    stepwise: bool,
    /// Moves the wall clock only: the Mac slept, and no timer noticed.
    #[serde(default)]
    jump_ms: Option<u64>,
}

/// Moves app time and returns it once the core has settled.
async fn clock(State(s): State<Arc<Server>>, Json(req): Json<ClockReq>) -> Ctl {
    if let Some(ms) = req.jump_ms {
        s.clock.jump(ms);
    }
    if let Some(mut left) = req.advance_ms {
        if req.stepwise {
            while let Some(wait) = s.clock.next_due_within(left) {
                s.clock.advance(wait);
                left -= wait;
                s.settle().await?;
            }
        }
        s.clock.advance(left);
        s.settle().await?;
    }
    Ok(Json(json!({ "now": crate::clock::now_ms() })))
}

#[derive(Deserialize)]
struct FocusReq {
    focused: bool,
}

/// The app window gains or loses focus. Focus soon after a background
/// notification counts as a click on it: the page gets `notification-open`, as
/// the app's webview does.
async fn focus(State(s): State<Arc<Server>>, Json(req): Json<FocusReq>) -> Ctl {
    let Some(e) = s.engine().await else {
        return Err(Fail("the core is not running".into()));
    };
    if !req.focused {
        e.on_blur();
        return Ok(done());
    }
    let Some(n) = e.on_focus() else {
        return Ok(Json(json!({ "openThread": null })));
    };
    s.bridge.emit(
        "notification-open",
        json!({ "account": n.account, "thread": n.thread }),
    );
    let thread = match n.thread {
        Some(t) => s.gmail_thread(t).await,
        None => None,
    };
    Ok(Json(json!({
        "openThread": { "account": n.account_email, "thread": thread },
    })))
}

async fn platform(State(s): State<Arc<Server>>) -> Ctl {
    let (notifications, badge, opened_urls, opened_files, focus_requests) = {
        let r = s.platform.recorded.lock();
        (
            r.notifications.clone(),
            r.badge,
            r.opened_urls.clone(),
            r.opened_files.clone(),
            r.focus_requests,
        )
    };
    let mut list = Vec::with_capacity(notifications.len());
    for n in notifications {
        let thread = match n.thread {
            Some(t) => s.gmail_thread(t).await,
            None => None,
        };
        let account = s
            .account_email(n.account)
            .await
            .unwrap_or_else(|| n.account_email.clone());
        list.push(json!({
            "title": n.title,
            "body": n.text(),
            "account": account,
            "thread": thread,
        }));
    }
    Ok(Json(json!({
        "notifications": list,
        "badge": badge,
        "openedUrls": opened_urls,
        "openedFiles": opened_files,
        "focusRequests": focus_requests,
    })))
}

/// Forgets what the platform recorded so far (the badge stays as it is).
async fn clear_recorded(State(s): State<Arc<Server>>) -> Ctl {
    let mut r = s.platform.recorded.lock();
    r.notifications.clear();
    r.opened_urls.clear();
    r.opened_files.clear();
    r.focus_requests = 0;
    Ok(done())
}

// ------------------------------------------------------------ the web

/// A 1×1 PNG.
const PIXEL: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0,
    1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54,
    0x78, 0x9C, 0x63, 0x60, 0x60, 0xF8, 0x0F, 0, 0x01, 0x04, 0x01, 0x00, 0x5F, 0xB3, 0x7E, 0x3B, 0,
    0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn hit(s: &Server, method: &Method, uri: &axum::http::Uri) {
    s.hits.lock().push(Hit {
        method: method.to_string(),
        path: uri.path().to_owned(),
        at: crate::clock::now_ms(),
    });
}

/// The remote image host: any `/img/<name>` is a pixel, and every fetch is logged.
async fn remote_image(
    State(s): State<Arc<Server>>,
    method: Method,
    uri: axum::http::Uri,
) -> Response {
    hit(&s, &method, &uri);
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        PIXEL,
    )
        .into_response()
}

/// A sink for hostile destinations (`/sink/evil.test/...`): logs and answers blank.
async fn sink(State(s): State<Arc<Server>>, method: Method, uri: axum::http::Uri) -> Response {
    hit(&s, &method, &uri);
    (
        [
            (header::CONTENT_TYPE, "text/html"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        "",
    )
        .into_response()
}

async fn hits(State(s): State<Arc<Server>>) -> Ctl {
    Ok(Json(json!(&*s.hits.lock())))
}

#[derive(Deserialize)]
struct ContainsReq {
    text: String,
}

/// Whether the database files hold `text` anywhere (SPEC Security: no tokens in SQLite).
async fn db_contains(State(s): State<Arc<Server>>, Query(q): Query<ContainsReq>) -> Ctl {
    let needle = q.text.as_bytes();
    if needle.is_empty() {
        return Err(Fail("empty text".into()));
    }
    let mut found = false;
    for suffix in ["", "-wal", "-journal"] {
        let path = PathBuf::from(format!("{}{suffix}", s.db.display()));
        match tokio::fs::read(&path).await {
            Ok(bytes) => found |= bytes.windows(needle.len()).any(|w| w == needle),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(Json(json!({ "found": found })))
}

/// Serves the built UI; unknown paths get `index.html`.
async fn static_file(State(s): State<Arc<Server>>, uri: axum::http::Uri) -> Response {
    let rel = uri.path().trim_start_matches('/');
    let safe = !rel.split('/').any(|seg| seg == "..");
    let mut path = s.dist.join(if rel.is_empty() || !safe {
        "index.html"
    } else {
        rel
    });
    if !path.is_file() {
        path = s.dist.join("index.html");
    }
    match tokio::fs::read(&path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, mime_for(&path))], bytes).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "build the UI first: vp build").into_response(),
    }
}

fn mime_for(path: &FsPath) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}
