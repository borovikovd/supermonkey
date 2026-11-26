use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, Request, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN, HOST,
    ORIGIN, VARY,
};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_core::Stream;
use futures_util::stream;
use parking_lot::RwLock;
use serde_json::Value;
use tokio::sync::broadcast;

use crate::core::ChannelSink;
use crate::ipc::{AppEvent, ChannelMessage};

use super::Session;

const SSE_BUFFER: usize = 512;
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub enum SseFrame {
    Channel(String),
    Event { name: String, data: String },
}

#[derive(Clone)]
pub struct WebEvents {
    tx: broadcast::Sender<SseFrame>,
}

impl std::fmt::Debug for WebEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebEvents").finish_non_exhaustive()
    }
}

impl Default for WebEvents {
    fn default() -> Self {
        Self::new()
    }
}

impl WebEvents {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(SSE_BUFFER);
        Self { tx }
    }

    #[must_use]
    pub fn channel_sink(&self) -> ChannelSink {
        let tx = self.tx.clone();
        ChannelSink::new(move |msg: ChannelMessage| {
            if let Ok(json) = serde_json::to_string(&msg) {
                let _receivers = tx.send(SseFrame::Channel(json)).ok();
            }
        })
    }

    pub fn emit(&self, event: &AppEvent) {
        if let Ok((name, payload)) = event.parts()
            && let Ok(data) = serde_json::to_string(&payload)
        {
            let _receivers = self.tx.send(SseFrame::Event { name, data }).ok();
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<SseFrame> {
        self.tx.subscribe()
    }
}

#[derive(Clone)]
pub struct BridgeState {
    session: Arc<RwLock<Option<Session>>>,
    events: WebEvents,
    allowed_origins: Arc<[String]>,
}

impl std::fmt::Debug for BridgeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeState")
            .field("allowed_origins", &self.allowed_origins)
            .finish_non_exhaustive()
    }
}

impl BridgeState {
    #[must_use]
    pub fn new(session: Option<Session>, events: WebEvents, allowed_origins: Vec<String>) -> Self {
        Self {
            session: Arc::new(RwLock::new(session)),
            events,
            allowed_origins: Arc::from(allowed_origins),
        }
    }

    pub fn set_session(&self, session: Option<Session>) {
        if let Some(ref s) = session {
            s.core().set_channel(Some(self.events.channel_sink()));
        }
        *self.session.write() = session;
    }

    #[must_use]
    pub fn session(&self) -> Option<Session> {
        self.session.read().clone()
    }
}

pub fn router(state: BridgeState) -> Router {
    let guard_origins = Arc::clone(&state.allowed_origins);
    Router::new()
        .route("/events", get(sse_events))
        .route("/invoke/{command}", post(invoke_command))
        .with_state(state)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(move |req, next| {
            guard_request(Arc::clone(&guard_origins), req, next)
        }))
}

async fn sse_events(
    State(state): State<BridgeState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = state.events.subscribe();
    let stream = stream::unfold((rx, false), |(mut rx, sent_ready)| async move {
        if !sent_ready {
            return Some((Ok(Event::default().comment("ready")), (rx, true)));
        }
        loop {
            match rx.recv().await {
                Ok(SseFrame::Channel(json)) => {
                    return Some((Ok(Event::default().data(json)), (rx, true)));
                }
                Ok(SseFrame::Event { name, data }) => {
                    return Some((Ok(Event::default().event(name).data(data)), (rx, true)));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn invoke_command(
    State(state): State<BridgeState>,
    Path(command): Path<String>,
    Json(args): Json<Value>,
) -> Response {
    let Some(session) = state.session() else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json("app is not running")).into_response();
    };
    match session.dispatch(&command, args).await {
        Ok(reply) => (StatusCode::OK, Json(reply.into_json())).into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, Json(err)).into_response(),
    }
}

async fn guard_request(allowed_origins: Arc<[String]>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    if !is_loopback_host(headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !is_allowed_fetch_site(headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(origin) = check_origin(headers, &allowed_origins) else {
        return StatusCode::FORBIDDEN.into_response();
    };

    if req.method() == Method::OPTIONS {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        apply_cors(resp.headers_mut(), origin.as_deref());
        return resp;
    }

    let mut resp = next.run(req).await;
    apply_cors(resp.headers_mut(), origin.as_deref());
    resp
}

fn apply_cors(headers: &mut HeaderMap, origin: Option<&str>) {
    if let Some(orig) = origin
        && let Ok(val) = HeaderValue::from_str(orig)
    {
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, val);
        headers.insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        headers.insert(
            ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type"),
        );
        headers.insert(VARY, HeaderValue::from_static("Origin"));
    }
}

fn is_loopback_host(headers: &HeaderMap) -> bool {
    let Some(raw) = headers.get(HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let host = if let Some(rest) = raw.strip_prefix('[') {
        rest.split_once(']').map_or("", |(h, _)| h)
    } else {
        raw.split(':').next().unwrap_or("")
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn is_allowed_fetch_site(headers: &HeaderMap) -> bool {
    let Some(raw) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) else {
        return true;
    };
    matches!(raw, "same-origin" | "same-site" | "none")
}

fn check_origin(headers: &HeaderMap, allowed: &[String]) -> Result<Option<String>, ()> {
    let Some(raw) = headers.get(ORIGIN).and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    if allowed.iter().any(|a| a == raw) {
        return Ok(Some(raw.to_owned()));
    }
    if let Some(host_hdr) = headers.get(HOST).and_then(|v| v.to_str().ok()) {
        let same_http = format!("http://{host_hdr}");
        if raw == same_http {
            return Ok(Some(raw.to_owned()));
        }
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    #[expect(
        clippy::too_many_lines,
        reason = "verifies host, origin, Sec-Fetch-Site, CORS preflight, and loopback pass-through"
    )]
    async fn bridge_rejects_non_loopback_hosts_and_foreign_origins() {
        let state = BridgeState::new(
            None,
            WebEvents::new(),
            vec!["http://localhost:5173".to_owned()],
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ok = axum::serve(listener, router(state)).await.is_ok();
        });

        let client = reqwest::Client::new();
        let url = format!("http://{addr}/invoke/connect");

        let r = client
            .post(&url)
            .header("host", "evil.test:1430")
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        let r = client
            .post(&url)
            .header("origin", "https://evil.test")
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        let r = client
            .post(&url)
            .header("origin", "http://localhost.evil.test")
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        let r = client
            .post(&url)
            .header("sec-fetch-site", "cross-site")
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN);

        let r = client
            .request(Method::OPTIONS, &url)
            .header("origin", "http://localhost:5173")
            .header("sec-fetch-site", "same-site")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            r.headers()
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("http://localhost:5173")
        );

        let r = client
            .post(&url)
            .header("origin", format!("http://{addr}"))
            .header("sec-fetch-site", "same-origin")
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);

        server.abort();
    }
}
