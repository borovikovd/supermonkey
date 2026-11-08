//! The debug web bridge (ARCHITECTURE.md, section 13): the command surface
//! over HTTP, so the UI can run in a browser against the real core.
//!
//! - `POST /invoke/{cmd}` runs a command through [`crate::dispatch`] and
//!   answers its JSON result, or `400` with the error string.
//! - `GET /events` streams server-sent events to one page: each channel
//!   message as a plain event, and each app event (Tauri's `emit` in the app)
//!   as a named one. A new stream takes the channel from the page before.
//!
//! The debug app serves it on `LOCAL_ADDR` when `SUPERMONKEY_WEB_BRIDGE=1`;
//! the E2E server mounts [`WebBridge::router`] on its own origin. Release
//! builds never enable the `web` feature.
//!
//! The bridge serves real mail in a debug app, so it answers only requests
//! addressed to a loopback host (no DNS rebinding) from its own origin or
//! the Vite dev server's: any other page in the browser gets `403`. That
//! includes a page's `<img>` or `<script>` pointed at `/events`, which sends
//! no `Origin` but a `Sec-Fetch-Site` that is not `same-origin`: opening the
//! stream would take the channel from the app's page.

use std::convert::Infallible;
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

/// Where the debug app serves the bridge, for `vp dev` in a browser.
pub(crate) const LOCAL_ADDR: &str = "127.0.0.1:1430";

/// The Vite dev server's origins (`vite.config.ts`), whose page calls the
/// bridge across origins.
const DEV_ORIGINS: [&str; 2] = ["http://localhost:5173", "http://127.0.0.1:5173"];

/// The largest invoke body: a file at the attachment limit, in base64, with
/// room for the JSON around it. Axum's own default (2 MB) refused larger
/// files before the core could judge them.
const BODY_LIMIT: usize = MAX_ATTACHMENT_BYTES.div_ceil(3) * 4 + 1024 * 1024;

/// The bridge's state: the core it serves, which the E2E server swaps when
/// it restarts the core, and the pages it streams to.
#[derive(Clone)]
pub struct WebBridge(Arc<Shared>);

struct Shared {
    app: RwLock<Option<Arc<App>>>,
    /// The core's activity: a command counts as busy while it runs.
    activity: Arc<Activity>,
    /// One sender per open `/events` stream.
    streams: Mutex<Vec<UnboundedSender<Push>>>,
    /// `rev` and `ack` of the newest channel message sent to a page.
    sent: Mutex<(Rev, ActionNo)>,
    /// Woken when the page calls `quit_now`.
    quit: Notify,
    /// Sign-ins running now; one waiting on the person counts too.
    sign_ins: AtomicUsize,
}

/// What goes down an event stream.
enum Push {
    Message(ChannelMessage),
    AppEvent { name: String, payload: Value },
}

impl WebBridge {
    /// A bridge serving `app` (none while the core is down), counting its
    /// commands in `activity`.
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

    /// The core served now.
    pub fn app(&self) -> Option<Arc<App>> {
        self.0.app.read().clone()
    }

    /// The core to serve; `None` while it is down, which also ends the open
    /// event streams once the core's writer has let go of their channel.
    pub fn set_app(&self, app: Option<Arc<App>>) {
        if app.is_none() {
            self.0.streams.lock().clear();
        }
        *self.0.app.write() = app;
    }

    /// Sends an app event to every open page, as `AppHandle::emit` does in
    /// the app. Returns how many pages got it.
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

    /// Sign-ins running now.
    pub fn sign_ins(&self) -> usize {
        self.0.sign_ins.load(Ordering::Acquire)
    }

    /// `rev` and `ack` of the newest channel message sent to a page.
    pub fn last_sent(&self) -> (Rev, ActionNo) {
        *self.0.sent.lock()
    }

    /// Resolves when a page next calls `quit_now`. Enable it before asking
    /// the page to quit, so a quick answer is not missed.
    pub fn quit_called(&self) -> Notified<'_> {
        self.0.quit.notified()
    }

    /// The bridge's routes, with CORS for the dev server's origin.
    pub fn router(&self) -> Router {
        Router::new()
            .route("/events", get(events))
            .route("/invoke/{cmd}", post(invoke).options(preflight))
            .layer(DefaultBodyLimit::max(BODY_LIMIT))
            .layer(middleware::from_fn(local_only))
            .with_state(self.clone())
    }
}

/// Serves the bridge on 127.0.0.1:1430 (`LOCAL_ADDR`) until the future is
/// dropped.
pub async fn serve_local(bridge: WebBridge) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(LOCAL_ADDR).await?;
    tracing::info!(addr = LOCAL_ADDR, "web bridge listening");
    axum::serve(listener, bridge.router()).await
}

/// The browser's word on who made a request: `same-origin`, `same-site`,
/// `cross-site`, or `none` (typed in the address bar). Tools send none.
const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");

/// Whether a request may reach the bridge: addressed to a loopback host,
/// and from no page (`curl`), the bridge's own page (a same-origin
/// `EventSource`, which sends no `Origin`), the bridge's own origin, or the
/// dev server's. A request with no `Origin` that the browser says another
/// page made (`Sec-Fetch-Site`) is refused.
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

/// Refuses requests [`is_allowed`] does not let in, and answers the dev
/// server's page with CORS headers for its own origin.
async fn local_only(request: Request, next: Next) -> Response {
    let headers = request.headers();
    // Kept past the request, for the response's CORS headers.
    let origin = headers.get(ORIGIN).filter(|v| v.to_str().is_ok()).cloned();
    let allowed = is_allowed(
        header(headers, HOST),
        origin.as_ref().and_then(|o| o.to_str().ok()),
        header(headers, SEC_FETCH_SITE),
    );
    if !allowed {
        tracing::warn!("the web bridge refused a request from another page or host");
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = next.run(request).await;
    if let Some(origin) = origin {
        let headers = response.headers_mut();
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
    response
}

/// A request header's value as text.
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

/// Streams channel messages and app events to one page. A page whose stream
/// reconnects reloads, since it missed what was sent meanwhile.
async fn events(State(bridge): State<WebBridge>) -> Response {
    let Some(app) = bridge.app() else {
        return down();
    };
    let (page, stream) = unbounded_channel();
    let to_page = page.clone();
    let shared = Arc::clone(&bridge.0);
    let sink = Box::new(move |message: ChannelMessage| {
        *shared.sent.lock() = (message.rev, message.ack);
        // The page has gone; the next stream takes the channel.
        let _ = to_page.send(Push::Message(message));
    });
    if let Err(e) = app.lifecycle().connect(sink).await {
        tracing::warn!(error = %e, "couldn't connect a page's event stream");
        return down();
    }
    bridge.0.streams.lock().push(page);
    Sse::new(PushStream(stream)).into_response()
}

/// An event stream's pushes as server-sent events.
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

/// Whether a command runs a sign-in, which may wait on the person in the
/// browser for as long as they take.
fn is_sign_in(command: &str) -> bool {
    matches!(command, "add_account" | "sign_in_again")
}

/// Runs a command by name, as the app's Tauri `dispatch` command does. It counts as core
/// activity until it returns, except while a sign-in waits on the person.
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
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!(e))).into_response(),
    }
}

/// Counts a sign-in while its command runs.
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
        // The dev server's page, and the E2E server's page on its own origin.
        assert!(is_allowed(
            Some("127.0.0.1:1430"),
            Some("http://localhost:5173"),
            None
        ));
        assert!(is_allowed(
            Some("127.0.0.1:1430"),
            Some("http://127.0.0.1:5173"),
            None
        ));
        assert!(is_allowed(
            Some("127.0.0.1:41234"),
            Some("http://127.0.0.1:41234"),
            None
        ));
        assert!(is_allowed(Some("localhost:1430"), None, None));
        // Any other page in the browser.
        assert!(!is_allowed(
            Some("127.0.0.1:1430"),
            Some("https://evil.test"),
            None
        ));
        assert!(!is_allowed(
            Some("127.0.0.1:1430"),
            Some("http://127.0.0.1:9999"),
            None
        ));
        assert!(!is_allowed(Some("127.0.0.1:1430"), Some("null"), None));
        // DNS rebinding: a hostile name that resolves to 127.0.0.1.
        assert!(!is_allowed(Some("evil.test:1430"), None, None));
        assert!(!is_allowed(
            Some("evil.test:1430"),
            Some("http://evil.test:1430"),
            None
        ));
        assert!(!is_allowed(None, None, None));
        // The bridge's own page: a same-origin `EventSource` sends no
        // `Origin`, and its browser says so.
        assert!(is_allowed(
            Some("127.0.0.1:41234"),
            None,
            Some("same-origin")
        ));
        assert!(is_allowed(Some("127.0.0.1:1430"), None, Some("none")));
        // Another page's `<img src=…/events>`: no `Origin`, but the browser
        // says another page made it.
        assert!(!is_allowed(
            Some("127.0.0.1:1430"),
            None,
            Some("cross-site")
        ));
        assert!(!is_allowed(Some("localhost:1430"), None, Some("same-site")));
    }
}
