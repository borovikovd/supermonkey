//! The debug web bridge (ARCHITECTURE.md, section 13): the command surface
//! over HTTP, so the UI can run in a browser against the real core.
//!
//! - `POST /invoke/{cmd}` runs a command through [`crate::dispatch`] and
//!   answers its JSON result, or `400` with the error string.
//! - `GET /events` streams server-sent events to one page: each channel
//!   message as a plain event, and each app event (Tauri's `emit` in the app)
//!   as a named one. A new stream takes the channel from the page before.
//!
//! The debug app serves it on [`LOCAL_ADDR`] when `UBERMENSCH_WEB_BRIDGE=1`;
//! the E2E server mounts [`WebBridge::router`] on its own origin. Release
//! builds never enable the `web` feature.

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_core::Stream;
use parking_lot::{Mutex, RwLock};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::sync::futures::Notified;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use ub_core::App;
use ub_core::activity::{Activity, busy_scope};
use ub_domain::{ActionNo, Rev};
use ub_ipc::ChannelMessage;

use crate::{Reply, dispatch, is_sign_in};

/// Where the debug app serves the bridge, for `vp dev` in a browser.
pub const LOCAL_ADDR: &str = "127.0.0.1:1430";

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
    AppEvent { name: &'static str, payload: Value },
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
    pub fn emit(&self, name: &'static str, payload: &Value) -> usize {
        let mut streams = self.0.streams.lock();
        streams.retain(|page| {
            page.send(Push::AppEvent {
                name,
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
            .with_state(self.clone())
    }
}

/// Serves the bridge on [`LOCAL_ADDR`] until the future is dropped.
pub async fn serve_local(bridge: WebBridge) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(LOCAL_ADDR).await?;
    tracing::info!(addr = LOCAL_ADDR, "web bridge listening");
    axum::serve(listener, bridge.router()).await
}

fn cors(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    headers.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("content-type"),
    );
    headers.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("POST, GET, OPTIONS"),
    );
    response
}

async fn preflight() -> Response {
    cors(StatusCode::NO_CONTENT.into_response())
}

fn down() -> Response {
    cors(
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!("the core is not running")),
        )
            .into_response(),
    )
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
    cors(Sse::new(PushStream(stream)).into_response())
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

/// Runs a command by name, as the Tauri commands do. It counts as core
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
    cors(match result {
        Ok(Reply::Value(value)) => Json(value).into_response(),
        Ok(Reply::Quit) => {
            bridge.0.quit.notify_waiters();
            Json(Value::Null).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!(e))).into_response(),
    })
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
