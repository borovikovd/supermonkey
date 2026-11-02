//! The E2E server: the real core and the web bridge against a fake Gmail,
//! plus the built UI and a `/__test` control API, all on one origin.
//!
//! Lifecycle: the process starts the fake Gmail and an empty database, then
//! waits. A test seeds accounts and mail through `/__test`, calls
//! `POST /__test/start` to launch the engine, and only then loads the page.

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;
use ub_fake_gmail::{FakeGmail, FakeUser, Fault, Matcher, NewAttachment, NewMessage};
use ub_gmail::{MemoryTokenStore, OAuthConfig, QuotaConfig, TokenStore};

use super::{Config, Engine, EngineRef, Platform};
use crate::db::ReadPool;
use crate::writer::{Effect, Writer};

/// What the app asked of the OS, for tests to assert on.
#[derive(Default, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Recorded {
    notifications: Vec<RecordedNotification>,
    badge: Option<i64>,
    opened_urls: Vec<String>,
    opened_files: Vec<String>,
    focus_requests: usize,
}

#[derive(Serialize, Clone)]
struct RecordedNotification {
    title: String,
    body: String,
}

/// How the next Google sign-in goes.
#[derive(Clone, Default)]
enum SignIn {
    /// The fake signs in its first user.
    #[default]
    FirstUser,
    As(String),
    /// The user closes the browser tab: nothing comes back.
    Abandon,
}

/// Records every call. Opening the sign-in page "completes" it with a plain
/// HTTP request: the fake OAuth page approves at once and redirects to the
/// app's loopback listener.
struct TestPlatform {
    recorded: Mutex<Recorded>,
    sign_in: Mutex<SignIn>,
    http: reqwest::Client,
}

impl Platform for TestPlatform {
    fn open_url(&self, url: &str) {
        self.recorded.lock().opened_urls.push(url.to_owned());
        let mode = self.sign_in.lock().clone();
        let url = match mode {
            SignIn::Abandon => return,
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
        tokio::spawn(async move {
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

    fn notify(&self, title: &str, body: &str) {
        self.recorded
            .lock()
            .notifications
            .push(RecordedNotification {
                title: title.to_owned(),
                body: body.to_owned(),
            });
    }

    fn set_badge(&self, count: Option<i64>) {
        self.recorded.lock().badge = count;
    }

    fn focus_window(&self) {
        self.recorded.lock().focus_requests += 1;
    }
}

struct Server {
    fake: FakeGmail,
    users: Mutex<HashMap<String, FakeUser>>,
    store: Arc<MemoryTokenStore>,
    engine: EngineRef,
    platform: Arc<TestPlatform>,
    /// Taken by `/__test/start`.
    effects: Mutex<Option<UnboundedReceiver<Effect>>>,
    dist: PathBuf,
}

type Shared = Arc<Server>;

/// Runs the server until the process is killed. Prints `READY <url>` once listening.
pub async fn run(port: u16, dist: PathBuf) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let data_dir = dir.path().to_path_buf();
    let _ = crate::drafts::DRAFT_FILES.set(data_dir.join("drafts"));
    let db = data_dir.join("e2e.db");
    let (effects_tx, effects_rx) = tokio::sync::mpsc::unbounded_channel();
    let writer = Writer::start(&db, effects_tx)?;
    let reads = ReadPool::new(&db, 4);
    let fake = FakeGmail::start();
    let config = Config {
        gmail_base: fake.base_url(),
        oauth: OAuthConfig {
            client_id: "e2e".into(),
            client_secret: "e2e".into(),
            auth_url: fake.auth_url(),
            token_url: fake.token_url(),
            revoke_url: fake.revoke_url(),
        },
        quota: QuotaConfig::default(),
    };
    let store = Arc::new(MemoryTokenStore::default());
    let platform = Arc::new(TestPlatform {
        recorded: Mutex::new(Recorded::default()),
        sign_in: Mutex::new(SignIn::default()),
        http: reqwest::Client::new(),
    });
    let engine = Engine::new(
        platform.clone(),
        writer,
        reads,
        data_dir,
        config,
        store.clone(),
    );
    let server = Arc::new(Server {
        fake,
        users: Mutex::new(HashMap::new()),
        store,
        engine: engine.clone(),
        platform,
        effects: Mutex::new(Some(effects_rx)),
        dist,
    });
    let app = super::webbridge::router(engine).merge(control(server.clone()));
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

fn control(server: Shared) -> Router {
    Router::new()
        .route("/__test/account", post(add_account))
        .route("/__test/user", post(add_user))
        .route("/__test/start", post(start))
        .route("/__test/deliver", post(deliver))
        .route("/__test/gmail/{email}", get(gmail_state))
        .route("/__test/gmail/{email}/modify", post(gmail_modify))
        .route("/__test/gmail/{email}/delete", post(gmail_delete))
        .route("/__test/gmail/{email}/label", post(gmail_label))
        .route("/__test/gmail/{email}/send-as", post(gmail_send_as))
        .route("/__test/invalid-grant", post(invalid_grant))
        .route("/__test/sign-in", post(next_sign_in))
        .route("/__test/fault", post(fault))
        .route("/__test/clear-faults", post(clear_faults))
        .route("/__test/offline", post(offline))
        .route("/__test/requests", get(requests))
        .route("/__test/poll", post(poll))
        .route("/__test/clock", post(clock))
        .route("/__test/focus", post(focus))
        .route("/__test/platform", get(platform))
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
async fn add_user(State(s): State<Shared>, Json(req): Json<UserReq>) -> Ctl {
    create_user(&s, &req);
    Ok(done())
}

/// A Gmail user already signed in to the app (before `/__test/start`).
async fn add_account(State(s): State<Shared>, Json(req): Json<UserReq>) -> Ctl {
    let u = create_user(&s, &req);
    s.store
        .store_refresh_token(&req.email, u.refresh_token())
        .map_err(|e| Fail(e.0))?;
    let email = req.email.clone();
    let id = s
        .engine
        .writer
        .call(move |ctx| crate::accounts::create(ctx, &email))
        .await?;
    Ok(Json(json!({ "id": id })))
}

async fn start(State(s): State<Shared>) -> Ctl {
    let effects = s
        .effects
        .lock()
        .take()
        .ok_or_else(|| Fail("already started".into()))?;
    s.engine.run(effects);
    Ok(done())
}

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
    list_id: Option<String>,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    attachments: Vec<AttachmentReq>,
    /// Unix ms; defaults to app time.
    #[serde(default)]
    date: Option<u64>,
    /// Poll at once, so the app sees the message without waiting.
    #[serde(default = "yes")]
    poll: bool,
}

fn yes() -> bool {
    true
}

/// Puts a message in a mailbox, as if it arrived or was filed in Gmail web.
async fn deliver(State(s): State<Shared>, Json(req): Json<DeliverReq>) -> Ctl {
    let u = user(&s, &req.account)?;
    let attachments = req
        .attachments
        .into_iter()
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
        .collect::<Result<Vec<_>, base64::DecodeError>>()?;
    let date = req
        .date
        .unwrap_or_else(|| u64::try_from(crate::db::now_ms()).unwrap_or(0));
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
        list_id: req.list_id,
        extra_headers: req.headers,
        attachments,
        internal_date: Some(date),
        ..Default::default()
    });
    if req.poll {
        s.engine.kick_polls();
    }
    Ok(Json(json!({
        "id": added.id,
        "threadId": added.thread_id,
        "messageId": added.rfc822_message_id,
    })))
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

fn message_view(m: &ub_fake_gmail::FakeMessage) -> MessageView {
    MessageView {
        id: m.id.clone(),
        thread_id: m.thread_id.clone(),
        labels: m.label_ids.clone(),
        date: m.internal_date,
        snippet: m.snippet.clone(),
        headers: m
            .headers
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.clone()))
            .collect(),
        raw: String::from_utf8_lossy(&m.raw).into_owned(),
    }
}

/// The mailbox as Gmail sees it: messages, drafts, and labels.
async fn gmail_state(State(s): State<Shared>, Path(email): Path<String>) -> Ctl {
    let u = user(&s, &email)?;
    let messages: Vec<MessageView> = u.messages().iter().map(message_view).collect();
    let drafts: Vec<Value> = u
        .drafts()
        .iter()
        .map(|d| json!({ "id": d.id, "message": message_view(&d.message) }))
        .collect();
    let labels: Vec<Value> = u
        .labels()
        .into_iter()
        .map(|(id, name)| json!({ "id": id, "name": name }))
        .collect();
    Ok(Json(json!({
        "messages": messages,
        "drafts": drafts,
        "labels": labels,
        "historyId": u.history_id(),
    })))
}

#[derive(Deserialize)]
struct ModifyReq {
    id: String,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

/// A label change made outside the app (Gmail web, a filter).
async fn gmail_modify(
    State(s): State<Shared>,
    Path(email): Path<String>,
    Json(req): Json<ModifyReq>,
) -> Ctl {
    let u = user(&s, &email)?;
    let add: Vec<&str> = req.add.iter().map(String::as_str).collect();
    let remove: Vec<&str> = req.remove.iter().map(String::as_str).collect();
    u.modify_labels(&req.id, &add, &remove);
    s.engine.kick_polls();
    Ok(done())
}

#[derive(Deserialize)]
struct IdReq {
    id: String,
}

/// A message deleted outside the app.
async fn gmail_delete(
    State(s): State<Shared>,
    Path(email): Path<String>,
    Json(req): Json<IdReq>,
) -> Ctl {
    user(&s, &email)?.delete_message(&req.id);
    s.engine.kick_polls();
    Ok(done())
}

#[derive(Deserialize)]
struct NameReq {
    name: String,
}

/// A label created outside the app. Returns its Gmail id.
async fn gmail_label(
    State(s): State<Shared>,
    Path(email): Path<String>,
    Json(req): Json<NameReq>,
) -> Ctl {
    let id = user(&s, &email)?.add_label(&req.name);
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendAsReq {
    email: String,
    display_name: String,
}

async fn gmail_send_as(
    State(s): State<Shared>,
    Path(email): Path<String>,
    Json(req): Json<SendAsReq>,
) -> Ctl {
    user(&s, &email)?.add_send_as(&req.email, &req.display_name);
    Ok(done())
}

#[derive(Deserialize)]
struct InvalidGrantReq {
    email: String,
    on: bool,
}

/// Google revokes (or restores) the account's refresh token.
async fn invalid_grant(State(s): State<Shared>, Json(req): Json<InvalidGrantReq>) -> Ctl {
    let u = user(&s, &req.email)?;
    u.set_invalid_grant(req.on);
    if req.on {
        s.fake.expire_access_tokens();
    }
    s.engine.kick_polls();
    Ok(done())
}

#[derive(Deserialize)]
struct SignInReq {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    abandon: bool,
}

/// How the next sign-in in the browser goes.
async fn next_sign_in(State(s): State<Shared>, Json(req): Json<SignInReq>) -> Ctl {
    *s.platform.sign_in.lock() = match (req.abandon, req.email) {
        (true, _) => SignIn::Abandon,
        (false, Some(email)) => SignIn::As(email),
        (false, None) => SignIn::FirstUser,
    };
    Ok(done())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FaultReq {
    #[serde(default)]
    method: Option<String>,
    /// A glob on the request path, e.g. `*/messages/batchModify`.
    path: String,
    #[serde(default)]
    nth: Option<usize>,
    #[serde(default)]
    times: Option<usize>,
    /// `drop`, `noResponse`, `status`, or `delay`.
    kind: String,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    retry_after_secs: Option<u64>,
    #[serde(default)]
    delay_ms: Option<u64>,
}

/// Makes matching Gmail requests fail (SPEC Gmail errors).
async fn fault(State(s): State<Shared>, Json(req): Json<FaultReq>) -> Ctl {
    let mut m = Matcher::new(req.method.as_deref().unwrap_or("*"), &req.path);
    if let Some(n) = req.nth {
        m = m.nth(n);
    }
    if let Some(t) = req.times {
        m = m.times(t);
    }
    let f = match req.kind.as_str() {
        "drop" => Fault::Drop,
        "noResponse" => Fault::NoResponse,
        "status" => Fault::Status {
            status: req
                .status
                .ok_or_else(|| Fail("status needs `status`".into()))?,
            reason: req.reason.unwrap_or_else(|| "failedPrecondition".into()),
            retry_after_secs: req.retry_after_secs,
        },
        "delay" => Fault::Delay(Duration::from_millis(req.delay_ms.unwrap_or(1000))),
        other => return Err(Fail(format!("unknown fault kind {other}"))),
    };
    s.fake.inject(m, f);
    Ok(done())
}

async fn clear_faults(State(s): State<Shared>) -> Ctl {
    s.fake.clear_faults();
    Ok(done())
}

#[derive(Deserialize)]
struct OnReq {
    on: bool,
}

/// The network goes away (every Gmail request fails to connect) or comes back.
async fn offline(State(s): State<Shared>, Json(req): Json<OnReq>) -> Ctl {
    s.fake.set_offline(req.on).await;
    if !req.on {
        s.engine.kick_polls();
        s.engine.kick_queues();
    }
    Ok(done())
}

#[derive(Deserialize)]
struct RequestsQuery {
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

/// Gmail requests the fake received, optionally filtered.
async fn requests(State(s): State<Shared>, Query(q): Query<RequestsQuery>) -> Ctl {
    let count = s.fake.count(
        q.method.as_deref().unwrap_or("*"),
        q.path.as_deref().unwrap_or("*"),
    );
    let list: Vec<Value> = s
        .fake
        .requests()
        .into_iter()
        .map(|r| json!({ "method": r.method, "path": r.path, "query": r.query }))
        .collect();
    Ok(Json(json!({ "count": count, "requests": list })))
}

async fn poll(State(s): State<Shared>) -> Ctl {
    s.engine.kick_polls();
    Ok(done())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClockReq {
    advance_ms: i64,
}

/// Moves app time forward and wakes every timer that depends on it.
async fn clock(State(s): State<Shared>, Json(req): Json<ClockReq>) -> Ctl {
    crate::db::advance_clock(req.advance_ms);
    s.engine.snooze_kick.notify_one();
    s.engine.kick_queues();
    Ok(Json(json!({ "now": crate::db::now_ms() })))
}

#[derive(Deserialize)]
struct FocusReq {
    focused: bool,
}

/// The app window gains or loses focus (notification rules depend on it).
async fn focus(State(s): State<Shared>, Json(req): Json<FocusReq>) -> Ctl {
    if req.focused {
        let opened = s.engine.on_focus();
        Ok(Json(json!({
            "openThread": opened.map(|n| json!({ "account": n.account, "thread": n.thread })),
        })))
    } else {
        s.engine.on_blur();
        Ok(done())
    }
}

async fn platform(State(s): State<Shared>) -> Ctl<Recorded> {
    Ok(Json(s.platform.recorded.lock().clone()))
}

/// Serves the built UI; unknown paths get `index.html`.
async fn static_file(State(s): State<Shared>, uri: axum::http::Uri) -> Response {
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
