use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path as AxumPath, State};
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use parking_lot::Mutex;
use serde::Serialize;
use sm_fake_gmail::{FakeGmail, FakeUser};
use tempfile::TempDir;

use crate::core::{Clock, Core, CoreConfig, CoreError, Notification, Platform, log_err};
use crate::domain::{Millis, ThreadId};
use crate::gmail::{HttpClient, OAuthConfig, QuotaConfig, TokenFile, Units};
use crate::ipc::AppEvent;
use crate::session::web::{BridgeState, WebEvents, router as web_router};
use crate::session::{Host, Session};
use crate::store::{Priority as StorePriority, StoreError};

use super::control;

const FAST_QUOTA: QuotaConfig = QuotaConfig {
    capacity: Units(1_000_000),
    refill_per_minute: Units(60_000_000),
    background_per_minute: Units(60_000_000),
    background_reserve: Units(0),
    max_in_flight: 16,
};

const PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedNotification {
    pub title: String,
    pub body: String,
    pub account: String,
    pub thread: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordedEvent {
    pub event: String,
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedState {
    pub notifications: Vec<RecordedNotification>,
    pub badge: Option<u32>,
    pub opened_urls: Vec<String>,
    pub opened_files: Vec<String>,
    pub windows_shown: u32,
    pub events: Vec<RecordedEvent>,
    pub exits: u32,
}

#[derive(Debug, Clone, Default)]
pub struct SignInDirective {
    pub email: Option<String>,
    pub abandon: bool,
    pub deny: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordedHit {
    pub method: String,
    pub path: String,
    pub body: String,
    pub at: i64,
}

pub struct ServerState {
    pub data_dir: PathBuf,
    pub dist_dir: PathBuf,
    pub csp: String,
    pub fake: FakeGmail,
    pub clock: Clock,
    pub users: Mutex<HashMap<String, FakeUser>>,
    pub quota_production: AtomicBool,
    pub offline: AtomicBool,
    pub recorded: Mutex<RecordedState>,
    pub next_sign_in: Mutex<Option<SignInDirective>>,
    pub hits: Mutex<Vec<RecordedHit>>,
    pub events: WebEvents,
    pub bridge: BridgeState,
    pub http: HttpClient,
}

impl std::fmt::Debug for ServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerState")
            .field("data_dir", &self.data_dir)
            .finish_non_exhaustive()
    }
}

impl ServerState {
    #[must_use]
    pub fn tokens(&self) -> TokenFile {
        TokenFile::new(self.data_dir.join("tokens.json"))
    }

    #[must_use]
    pub fn user(&self, email: &str) -> Option<FakeUser> {
        self.users
            .lock()
            .get(&email.trim().to_ascii_lowercase())
            .cloned()
    }

    pub fn ensure_user(&self, email: &str, name: Option<&str>) -> FakeUser {
        let key = email.trim().to_ascii_lowercase();
        let mut map = self.users.lock();
        if let Some(existing) = map.get(&key) {
            if let Some(display) = name {
                existing.set_display_name(display);
            }
            return existing.clone();
        }
        let user = self.fake.add_user(email);
        if let Some(display) = name {
            user.set_display_name(display);
        }
        map.insert(key, user.clone());
        user
    }

    #[must_use]
    pub fn core_config(&self) -> CoreConfig {
        let quota = if self.quota_production.load(Ordering::Acquire) {
            QuotaConfig::for_budget(Units(5800))
        } else {
            FAST_QUOTA
        };
        CoreConfig {
            data_dir: self.data_dir.clone(),
            gmail_base_url: self.fake.base_url(),
            oauth: OAuthConfig {
                client_id: "e2e-client".to_owned(),
                client_secret: "e2e-secret".to_owned(),
                auth_url: self.fake.auth_url(),
                token_url: self.fake.token_url(),
                revoke_url: self.fake.revoke_url(),
            },
            quota,
            clock: self.clock.clone(),
        }
    }

    pub async fn start_core(self: &Arc<Self>) -> Result<(), String> {
        let existing = self.bridge.session();
        if let Some(prev) = existing {
            prev.terminate().await;
            self.bridge.set_session(None);
        }
        let platform: Box<dyn Platform> = Box::new(TestPlatform {
            state: Arc::clone(self),
        });
        let core = Core::open(self.core_config(), platform).map_err(|e| e.to_string())?;
        if self.offline.load(Ordering::Acquire) {
            core.probe_network().await;
        }
        core.start().await.map_err(|e| e.to_string())?;
        let host: Box<dyn Host> = Box::new(TestHost {
            state: Arc::clone(self),
        });
        let session = Session::new(core, host);
        self.bridge.set_session(Some(session));
        Ok(())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "drains writer and waits for held-aware core quiescence with double-check yield"
    )]
    pub async fn wait_idle(&self, timeout: Duration) -> (i64, i64, usize, usize) {
        let Some(session) = self.bridge.session() else {
            return (0, 0, 0, self.fake.held());
        };
        let core = session.core();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut last_snap: Option<(i64, i64, usize, usize, usize, usize)> = None;
        let mut stable_ticks = 0u8;

        loop {
            let (rev, ack) = core
                .writer()
                .call(StorePriority::Background, |_tx, state| {
                    Ok::<_, StoreError>((state.sent_rev.0, state.ack.0))
                })
                .await
                .unwrap_or((0, 0));
            let held = self.fake.held();
            let in_flight = self.fake.in_flight();
            let busy = core.activity().busy_count();
            let sign_ins = core.activity().sign_in_count();

            let fast_idle = if held == 0 {
                busy == 0 && in_flight == 0
            } else {
                in_flight == held && busy <= held
            };

            if fast_idle {
                tokio::task::yield_now().await;
                let (rev2, ack2) = core
                    .writer()
                    .call(StorePriority::Background, |_tx, state| {
                        Ok::<_, StoreError>((state.sent_rev.0, state.ack.0))
                    })
                    .await
                    .unwrap_or((rev, ack));
                let held2 = self.fake.held();
                let in_flight2 = self.fake.in_flight();
                let busy2 = core.activity().busy_count();
                let still_idle = if held2 == 0 {
                    busy2 == 0 && in_flight2 == 0
                } else {
                    in_flight2 == held2 && busy2 <= held2
                };
                if still_idle && rev2 == rev && ack2 == ack {
                    return (rev2, ack2, core.activity().sign_in_count(), held2);
                }
                last_snap = None;
                stable_ticks = 0;
                continue;
            }

            if held > 0 && in_flight == held {
                let reqs = self.fake.requests().len();
                let snap = (rev, ack, busy, held, in_flight, reqs);
                if last_snap == Some(snap) {
                    stable_ticks = stable_ticks.saturating_add(1);
                    if stable_ticks >= 3 {
                        return (rev, ack, sign_ins, held);
                    }
                } else {
                    last_snap = Some(snap);
                    stable_ticks = 0;
                }
            } else {
                last_snap = None;
                stable_ticks = 0;
            }

            let now = tokio::time::Instant::now();
            if now >= deadline {
                return (rev, ack, sign_ins, held);
            }
            let step = (deadline - now).min(Duration::from_millis(5));
            if held == 0 {
                let _ = core.activity().wait_idle(step).await;
            } else {
                tokio::time::sleep(step).await;
            }
        }
    }

    pub fn lookup_gmail_thread(&self, thread: ThreadId) -> Option<String> {
        let session = self.bridge.session()?;
        session
            .core()
            .read_pool()
            .with(|db| db.thread_gmail_id(thread))
            .ok()
            .flatten()
            .map(|(_, gid)| gid.0)
    }

    pub fn lookup_account_email(&self, account: crate::domain::AccountId) -> Option<String> {
        let session = self.bridge.session()?;
        session
            .core()
            .read_pool()
            .with(|db| db.account(account))
            .ok()
            .flatten()
            .map(|a| a.email)
    }
}

struct TestPlatform {
    state: Arc<ServerState>,
}

impl Platform for TestPlatform {
    fn open_url(&self, url: &str) -> Result<(), CoreError> {
        self.state.recorded.lock().opened_urls.push(url.to_owned());

        let auth_prefix = self.state.fake.auth_url();
        if url.starts_with(&auth_prefix) {
            let directive = self.state.next_sign_in.lock().take().unwrap_or_default();
            if directive.abandon {
                return Ok(());
            }
            if directive.deny {
                self.state.fake.deny_next_sign_in();
            }
            let target_url = rewrite_login_hint(url, directive.email.as_deref());
            let http = self.state.http.clone();
            tokio::spawn(async move {
                log_err(
                    "drive fake OAuth browser redirect",
                    http.google().get(&target_url).send().await,
                );
            });
        }
        Ok(())
    }

    fn open_file(&self, path: &Path) -> Result<(), CoreError> {
        self.state
            .recorded
            .lock()
            .opened_files
            .push(path.to_string_lossy().into_owned());
        Ok(())
    }

    fn notify(&self, notification: Notification) {
        let gmail_thread = notification
            .thread
            .and_then(|tid| self.state.lookup_gmail_thread(tid));
        self.state
            .recorded
            .lock()
            .notifications
            .push(RecordedNotification {
                title: notification.title,
                body: notification.body,
                account: notification.account,
                thread: gmail_thread,
            });
    }

    fn set_badge(&self, count: u32) {
        self.state.recorded.lock().badge = (count > 0).then_some(count);
    }
}

fn rewrite_login_hint(raw_url: &str, email: Option<&str>) -> String {
    let Some(email) = email else {
        return raw_url.to_owned();
    };
    let Ok(mut parsed) = url::Url::parse(raw_url) else {
        return raw_url.to_owned();
    };
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| k != "login_hint")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    {
        let mut pairs = parsed.query_pairs_mut();
        pairs.clear();
        for (k, v) in kept {
            pairs.append_pair(&k, &v);
        }
        pairs.append_pair("login_hint", email);
    }
    parsed.into()
}

struct TestHost {
    state: Arc<ServerState>,
}

impl Host for TestHost {
    fn show_window(&self) {
        self.state.recorded.lock().windows_shown += 1;
    }

    fn emit(&self, event: AppEvent) {
        if let Ok((name, _)) = event.parts() {
            self.state
                .recorded
                .lock()
                .events
                .push(RecordedEvent { event: name });
        }
        self.state.events.emit(&event);
    }

    fn exit(&self) {
        self.state.recorded.lock().exits += 1;
    }
}

pub struct E2eServer {
    dir: TempDir,
    pub state: Arc<ServerState>,
}

impl std::fmt::Debug for E2eServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2eServer")
            .field("dir", &self.dir.path())
            .finish_non_exhaustive()
    }
}

impl E2eServer {
    pub fn new(start_ms: i64, dist_dir: PathBuf) -> std::io::Result<Self> {
        let dir = TempDir::new()?;
        let data_dir = dir.path().to_path_buf();
        let clock = Clock::manual(Millis(start_ms));
        let fake = FakeGmail::start();
        let clock_for_fake = clock.clone();
        fake.set_clock(move || u64::try_from(clock_for_fake.now().0).unwrap_or(0));

        let events = WebEvents::new();
        let bridge = BridgeState::new(None, events.clone(), Vec::new());
        let http = HttpClient::new().map_err(|e| std::io::Error::other(e.to_string()))?;

        let state = Arc::new(ServerState {
            data_dir,
            dist_dir,
            csp: release_csp_with_self(),
            fake,
            clock,
            users: Mutex::new(HashMap::new()),
            quota_production: AtomicBool::new(false),
            offline: AtomicBool::new(false),
            recorded: Mutex::new(RecordedState::default()),
            next_sign_in: Mutex::new(None),
            hits: Mutex::new(Vec::new()),
            events,
            bridge,
            http,
        });

        Ok(Self { dir, state })
    }

    pub fn router(&self) -> Router {
        let web = web_router(self.state.bridge.clone());
        let test_routes = control::router(Arc::clone(&self.state));
        let static_routes = Router::new()
            .route("/img/{*name}", get(serve_image))
            .route("/sink/{*rest}", any(serve_sink))
            .route("/", get(serve_index))
            .route("/{*path}", get(serve_asset))
            .with_state(Arc::clone(&self.state));
        Router::new()
            .merge(web)
            .merge(test_routes)
            .merge(static_routes)
    }

    pub async fn shutdown(&self) {
        if let Some(session) = self.state.bridge.session() {
            session.terminate().await;
            self.state.bridge.set_session(None);
        }
    }
}

fn release_csp_with_self() -> String {
    let raw = include_str!("../../tauri.conf.json");
    let csp = serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| {
            v.get("app")?
                .get("security")?
                .get("csp")?
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "default-src 'self'; script-src 'self'".to_owned());
    csp.replacen("connect-src ", "connect-src 'self' ", 1)
}

async fn serve_image(
    State(state): State<Arc<ServerState>>,
    AxumPath(name): AxumPath<String>,
) -> Response {
    let path = format!("/img/{name}");
    state.hits.lock().push(RecordedHit {
        method: "GET".to_owned(),
        path,
        body: String::new(),
        at: state.clock.now().0,
    });
    (
        StatusCode::OK,
        [(CONTENT_TYPE, HeaderValue::from_static("image/png"))],
        PIXEL_PNG,
    )
        .into_response()
}

async fn serve_sink(
    State(state): State<Arc<ServerState>>,
    method: Method,
    AxumPath(rest): AxumPath<String>,
    body: Bytes,
) -> Response {
    let path = format!("/sink/{rest}");
    let body_text = String::from_utf8_lossy(&body).into_owned();
    state.hits.lock().push(RecordedHit {
        method: method.to_string(),
        path,
        body: body_text,
        at: state.clock.now().0,
    });
    StatusCode::OK.into_response()
}

async fn serve_index(State(state): State<Arc<ServerState>>) -> Response {
    serve_file(&state, Path::new("index.html"))
}

async fn serve_asset(
    State(state): State<Arc<ServerState>>,
    AxumPath(rel): AxumPath<String>,
) -> Response {
    serve_file(&state, Path::new(&rel))
}

fn serve_file(state: &ServerState, rel: &Path) -> Response {
    let mut safe = PathBuf::new();
    for comp in rel.components() {
        match comp {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return StatusCode::FORBIDDEN.into_response();
            }
        }
    }
    let full = state.dist_dir.join(&safe);
    let Ok(bytes) = std::fs::read(&full) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = mime_for(&safe);
    let mut resp = (
        StatusCode::OK,
        [(CONTENT_TYPE, HeaderValue::from_static(mime))],
        bytes,
    )
        .into_response();
    if safe == Path::new("index.html")
        && let Ok(csp) = HeaderValue::from_str(&state.csp)
    {
        resp.headers_mut().insert(CONTENT_SECURITY_POLICY, csp);
    }
    resp
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

#[must_use]
pub fn resolve_dist_dir() -> PathBuf {
    let candidates = [
        PathBuf::from("dist"),
        PathBuf::from("../dist"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../dist"),
    ];
    for c in candidates {
        if c.join("index.html").is_file() {
            return c;
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../dist")
}
