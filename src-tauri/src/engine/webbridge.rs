//! The command surface over HTTP, so the UI can run in a browser against the
//! real Rust core: `POST /invoke/{cmd}` runs a command, `GET /events` streams
//! channel messages, and app events (Tauri's `emit`) as named SSE events.
//! Debug builds serve it on 127.0.0.1:1430 when `UBERMENSCH_WEB_BRIDGE=1`; the
//! E2E server mounts the same router.

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::{Path, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use parking_lot::{Mutex, RwLock};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;

use super::Engine;
use crate::api;
use crate::model::ChannelMessage;

/// What the bridge serves: the engine, which the E2E server swaps when it
/// restarts the core, and the app events it forwards to the page.
#[derive(Clone)]
pub struct Bridge(Arc<Shared>);

struct Shared {
    engine: RwLock<Option<Arc<Engine>>>,
    /// One sender per open `/events` stream.
    events: Mutex<Vec<UnboundedSender<UiEvent>>>,
    /// `(rev, ack)` of the newest channel message sent to the page.
    sent: Mutex<(i64, i64)>,
    /// Signalled when the page calls `quit_now`.
    quit: Notify,
    /// Sign-ins running now (`add_account`, `sign_in_again`).
    sign_ins: AtomicUsize,
}

/// An app event for the page, as Tauri's `emit` sends one.
#[derive(Clone)]
struct UiEvent {
    name: &'static str,
    payload: Value,
}

impl Bridge {
    pub fn new(engine: Option<Arc<Engine>>) -> Bridge {
        Bridge(Arc::new(Shared {
            engine: RwLock::new(engine),
            events: Mutex::new(Vec::new()),
            sent: Mutex::new((0, 0)),
            quit: Notify::new(),
            sign_ins: AtomicUsize::new(0),
        }))
    }

    pub fn engine(&self) -> Option<Arc<Engine>> {
        self.0.engine.read().clone()
    }

    /// The engine to serve; `None` while the core is down, which also ends
    /// the open event streams.
    pub fn set_engine(&self, engine: Option<Arc<Engine>>) {
        if engine.is_none() {
            self.0.events.lock().clear();
        }
        *self.0.engine.write() = engine;
    }

    /// Sends an app event to the page, as `AppHandle::emit` does in the app.
    /// Returns how many open pages got it.
    pub fn emit(&self, name: &'static str, payload: Value) -> usize {
        let event = UiEvent { name, payload };
        let mut events = self.0.events.lock();
        events.retain(|tx| tx.send(event.clone()).is_ok());
        events.len()
    }

    /// Sign-ins running now; one waiting on the browser counts too.
    pub fn sign_ins(&self) -> usize {
        self.0.sign_ins.load(Ordering::Acquire)
    }

    /// `(rev, ack)` of the newest channel message sent to the page.
    pub fn last_sent(&self) -> (i64, i64) {
        *self.0.sent.lock()
    }

    /// Notified (`notify_waiters`) when the page calls `quit_now`.
    pub fn quit_signal(&self) -> &Notify {
        &self.0.quit
    }
}

/// Serves the bridge on 127.0.0.1:1430 for the app's debug builds.
pub fn start(app: &AppHandle) {
    if !cfg!(debug_assertions) || std::env::var("UBERMENSCH_WEB_BRIDGE").as_deref() != Ok("1") {
        return;
    }
    let engine = app.state::<Arc<Engine>>().inner().clone();
    tauri::async_runtime::spawn(async move {
        match tokio::net::TcpListener::bind("127.0.0.1:1430").await {
            Ok(listener) => {
                log::info!("web bridge on http://127.0.0.1:1430");
                if let Err(e) = axum::serve(listener, router(Bridge::new(Some(engine)))).await {
                    log::warn!("web bridge stopped: {e}");
                }
            }
            Err(e) => log::warn!("web bridge: {e}"),
        }
    });
}

pub fn router(bridge: Bridge) -> Router {
    Router::new()
        .route("/events", get(events))
        .route("/invoke/{cmd}", post(invoke).options(preflight))
        .with_state(bridge)
}

fn cors(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    h.insert(
        "access-control-allow-headers",
        HeaderValue::from_static("content-type"),
    );
    h.insert(
        "access-control-allow-methods",
        HeaderValue::from_static("POST, GET, OPTIONS"),
    );
    r
}

async fn preflight(_m: Method) -> Response {
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

async fn events(State(bridge): State<Bridge>) -> Response {
    let Some(engine) = bridge.engine() else {
        return down();
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ChannelMessage>();
    let shared = bridge.0.clone();
    engine.writer.set_channel(move |m| {
        *shared.sent.lock() = (m.rev, m.ack);
        let _ = tx.send(m);
    });
    let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel::<UiEvent>();
    bridge.0.events.lock().push(events_tx);
    let messages = UnboundedReceiverStream::new(rx)
        .map(|m| Event::default().data(serde_json::to_string(&m).unwrap_or_default()));
    let app_events = UnboundedReceiverStream::new(events_rx)
        .map(|e| Event::default().event(e.name).data(e.payload.to_string()));
    let stream = messages.merge(app_events).map(Ok::<_, Infallible>);
    cors(Sse::new(stream).into_response())
}

fn arg<T: serde::de::DeserializeOwned>(v: &Value, k: &str) -> Result<T, String> {
    serde_json::from_value(v.get(k).cloned().unwrap_or(Value::Null))
        .map_err(|e| format!("{k}: {e}"))
}

/// Runs a command by name through [`crate::api`], as the Tauri commands do.
/// It counts as core activity until it returns.
async fn invoke(
    State(bridge): State<Bridge>,
    Path(cmd): Path<String>,
    Json(args): Json<Value>,
) -> Response {
    let Some(engine) = bridge.engine() else {
        return down();
    };
    let result = crate::clock::busy_scope("bridge", run(&bridge, &engine, &cmd, &args)).await;
    cors(match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!(e))).into_response(),
    })
}

async fn run(bridge: &Bridge, e: &Arc<Engine>, cmd: &str, args: &Value) -> Result<Value, String> {
    Ok(match cmd {
        // Unlike the Tauri command, events arrive on `/events` and no window is shown.
        "connect" => json!(api::launch_info(e)?),
        "submit" => {
            api::submit(e, arg(args, "actions")?);
            Value::Null
        }
        "fetch_rows" => json!(api::fetch_rows(
            e,
            arg(args, "drafts")?,
            &arg::<Vec<String>>(args, "keys")?,
        )?),
        "get_bodies" => json!(api::get_bodies(e, &arg::<Vec<_>>(args, "ids")?)?),
        "autocomplete" => json!(api::autocomplete(
            e,
            arg(args, "account")?,
            &arg::<String>(args, "query")?,
        )?),
        "snooze_options" => json!(api::snooze_options(&arg::<String>(args, "text")?)),
        "perf_report" => {
            api::perf_report(
                &arg::<String>(args, "action")?,
                arg(args, "ms")?,
                arg(args, "budget")?,
            );
            Value::Null
        }
        "add_account" => {
            let _signing_in = SignIn::new(bridge);
            json!(api::add_account(e).await?)
        }
        "sign_in_again" => {
            let _signing_in = SignIn::new(bridge);
            api::sign_in_again(e, arg(args, "account")?).await?;
            Value::Null
        }
        "open_attachment" => {
            api::open_attachment(e, arg(args, "attachment")?).await?;
            Value::Null
        }
        "open_url" => {
            api::open_url(e, &arg::<String>(args, "url")?)?;
            Value::Null
        }
        "quit_now" => {
            bridge.0.quit.notify_waiters();
            Value::Null
        }
        #[cfg(feature = "fake")]
        "demo_deliver" => {
            super::demo::deliver(&arg::<String>(args, "subject")?);
            e.kick_polls();
            Value::Null
        }
        "add_attachment" => json!(api::add_attachment(
            &arg::<String>(args, "draft")?,
            &arg::<String>(args, "filename")?,
            &arg::<String>(args, "mimeType")?,
            &arg::<String>(args, "data")?,
        )?),
        other => return Err(format!("unknown command {other}")),
    })
}

/// Counts a sign-in while its command runs.
struct SignIn<'a>(&'a Bridge);

impl<'a> SignIn<'a> {
    fn new(bridge: &'a Bridge) -> Self {
        bridge.0.sign_ins.fetch_add(1, Ordering::AcqRel);
        SignIn(bridge)
    }
}

impl Drop for SignIn<'_> {
    fn drop(&mut self) {
        self.0.0.sign_ins.fetch_sub(1, Ordering::AcqRel);
    }
}
