use std::convert::Infallible;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_core::Stream;
use parking_lot::{Mutex, RwLock};
use serde_json::{Value, json};
use sm_core::activity::{Activity, busy_scope};
use sm_core::{App, MAX_ATTACHMENT_BYTES};
use sm_domain::{ActionNo, Rev};
use sm_ipc::{AppEvent, ChannelMessage};
use tokio::sync::Notify;
use tokio::sync::futures::Notified;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::{Answer, dispatch};

const LOCAL_ADDR: &str = "127.0.0.1:1430";

const DEV_ORIGINS: [&str; 2] = ["http://localhost:5173", "http://127.0.0.1:5173"];

const BODY_LIMIT: usize = MAX_ATTACHMENT_BYTES.div_ceil(3) * 4 + 1024 * 1024;

#[derive(Clone)]
pub struct WebBridge(Arc<Shared>);

impl fmt::Debug for WebBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebBridge").finish_non_exhaustive()
    }
}

struct Shared {
    app: RwLock<Option<Arc<App>>>,
    activity: Arc<Activity>,
    streams: Mutex<Vec<UnboundedSender<Push>>>,
    sent: Mutex<(Rev, ActionNo)>,
    quit: Notify,
    sign_ins: AtomicUsize,
}

enum Push {
    Message(ChannelMessage),
    AppEvent { name: String, payload: Value },
}

impl WebBridge {
    pub fn new(app: Option<Arc<App>>, activity: Arc<Activity>) -> Self {
        Self(Arc::new(Shared {
            app: RwLock::new(app),
            activity,
            streams: Mutex::new(Vec::new()),
            sent: Mutex::new((Rev(0), ActionNo(0))),
            quit: Notify::new(),
            sign_ins: AtomicUsize::new(0),
        }))
    }

    #[must_use]
    pub fn app(&self) -> Option<Arc<App>> {
        self.0.app.read().clone()
    }

    pub fn set_app(&self, app: Option<Arc<App>>) {
        if app.is_none() {
            self.0.streams.lock().clear();
        }
        *self.0.app.write() = app;
    }

    pub fn emit(&self, event: &AppEvent) -> usize {
        let (name, payload) = match event.parts() {
            Ok(parts) => parts,
            Err(e) => {
                tracing::error!(error = %e, "couldn't encode an app event");
                return 0;
            }
        };
        let mut streams = self.0.streams.lock();
        streams.retain(|page| {
            page.send(Push::AppEvent {
                name: name.clone(),
                payload: payload.clone(),
            })
            .is_ok()
        });
        streams.len()
    }

    #[must_use]
    pub fn sign_ins(&self) -> usize {
        self.0.sign_ins.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn last_sent(&self) -> (Rev, ActionNo) {
        *self.0.sent.lock()
    }

    pub fn quit_called(&self) -> Notified<'_> {
        self.0.quit.notified()
    }

    pub fn router(&self) -> Router {
        Router::new()
            .route("/events", get(events))
            .route("/invoke/{cmd}", post(invoke).options(preflight))
            .layer(DefaultBodyLimit::max(BODY_LIMIT))
            .layer(middleware::from_fn(local_only))
            .with_state(self.clone())
    }
}

pub async fn serve_local(bridge: WebBridge) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(LOCAL_ADDR).await?;
    tracing::info!(addr = LOCAL_ADDR, "web bridge listening");
    axum::serve(listener, bridge.router()).await
}

const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");

fn is_allowed(host: Option<&str>, origin: Option<&str>, fetch_site: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let name = host.rsplit_once(':').map_or(host, |(name, _port)| name);
    if !matches!(name, "127.0.0.1" | "localhost") {
        return false;
    }
    match origin {
        Some(origin) => {
            DEV_ORIGINS.contains(&origin) || origin.strip_prefix("http://") == Some(host)
        }
        None => fetch_site.is_none_or(|site| matches!(site, "same-origin" | "none")),
    }
}

async fn local_only(request: Request, next: Next) -> Response {
    let headers = request.headers();
    let origin = headers.get(ORIGIN).filter(|v| v.to_str().is_ok()).cloned();
    let allowed = is_allowed(
        header(headers, HOST),
        origin.as_ref().and_then(|o| o.to_str().ok()),
        header(headers, SEC_FETCH_SITE),
    );
    if !allowed {
        return refused();
    }
    let mut response = next.run(request).await;
    if let Some(origin) = origin {
        allow_origin(response.headers_mut(), origin);
    }
    response
}

fn refused() -> Response {
    tracing::warn!("the web bridge refused a request from another page or host");
    StatusCode::FORBIDDEN.into_response()
}

fn allow_origin(headers: &mut HeaderMap, origin: HeaderValue) {
    headers.insert("access-control-allow-origin", origin);
    headers.insert("vary", HeaderValue::from_static("origin"));
    headers.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("content-type"),
    );
    headers.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("POST, GET, OPTIONS"),
    );
}

fn header(headers: &HeaderMap, name: HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

async fn preflight() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

fn down() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!("the core is not running")),
    )
        .into_response()
}

async fn events(State(bridge): State<WebBridge>) -> Response {
    let Some(app) = bridge.app() else {
        return down();
    };
    let (page, stream) = unbounded_channel();
    let to_page = page.clone();
    let shared = Arc::clone(&bridge.0);
    let sink = Box::new(move |message: ChannelMessage| {
        *shared.sent.lock() = (message.rev, message.ack);
        _ = to_page.send(Push::Message(message));
    });
    if let Err(e) = app.lifecycle().connect(sink).await {
        tracing::warn!(error = %e, "couldn't connect a page's event stream");
        return down();
    }
    bridge.0.streams.lock().push(page);
    Sse::new(PushStream(stream)).into_response()
}

struct PushStream(UnboundedReceiver<Push>);

impl Stream for PushStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let Some(push) = std::task::ready!(self.0.poll_recv(cx)) else {
                return Poll::Ready(None);
            };
            match push {
                Push::Message(message) => match serde_json::to_string(&message) {
                    Ok(data) => return Poll::Ready(Some(Ok(Event::default().data(data)))),
                    Err(e) => tracing::error!(error = %e, "couldn't encode a channel message"),
                },
                Push::AppEvent { name, payload } => {
                    return Poll::Ready(Some(Ok(Event::default()
                        .event(name)
                        .data(payload.to_string()))));
                }
            }
        }
    }
}

fn is_sign_in(command: &str) -> bool {
    matches!(command, "add_account" | "sign_in_again")
}

async fn invoke(
    State(bridge): State<WebBridge>,
    Path(cmd): Path<String>,
    Json(args): Json<Value>,
) -> Response {
    let Some(app) = bridge.app() else {
        return down();
    };
    let _signing_in = is_sign_in(&cmd).then(|| SignIn::new(&bridge.0.sign_ins));
    let result = busy_scope(&bridge.0.activity, "bridge", dispatch(&app, &cmd, args)).await;
    match result {
        Ok(Answer::Reply(reply)) => Json(reply.into_json()).into_response(),
        Ok(Answer::Quit) => {
            bridge.0.quit.notify_waiters();
            Json(Value::Null).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(e)).into_response(),
    }
}

struct SignIn<'a>(&'a AtomicUsize);

impl<'a> SignIn<'a> {
    fn new(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(count)
    }
}

impl Drop for SignIn<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::is_allowed;

    #[test]
    fn only_loopback_hosts_and_known_origins_reach_the_bridge() {
        for (host, origin, allowed) in [
            ("127.0.0.1:1430", "http://localhost:5173", true),
            ("127.0.0.1:1430", "http://127.0.0.1:5173", true),
            ("127.0.0.1:41234", "http://127.0.0.1:41234", true),
            ("127.0.0.1:1430", "https://evil.test", false),
            ("127.0.0.1:1430", "http://127.0.0.1:9999", false),
            ("127.0.0.1:1430", "null", false),
            ("evil.test:1430", "http://evil.test:1430", false),
        ] {
            assert_eq!(
                is_allowed(Some(host), Some(origin), None),
                allowed,
                "{host} {origin}"
            );
        }
    }

    #[test]
    fn without_an_origin_only_loopback_hosts_and_same_origin_fetches_reach_the_bridge() {
        for (host, fetch_site, allowed) in [
            (Some("localhost:1430"), None, true),
            (Some("evil.test:1430"), None, false),
            (None, None, false),
            (Some("127.0.0.1:41234"), Some("same-origin"), true),
            (Some("127.0.0.1:1430"), Some("none"), true),
            (Some("127.0.0.1:1430"), Some("cross-site"), false),
            (Some("localhost:1430"), Some("same-site"), false),
        ] {
            assert_eq!(
                is_allowed(host, None, fetch_site),
                allowed,
                "{host:?} {fetch_site:?}"
            );
        }
    }
}
