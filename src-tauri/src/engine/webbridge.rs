//! The command surface over HTTP, so the UI can run in a browser against the
//! real Rust core: `POST /invoke/{cmd}` runs a command, `GET /events` streams
//! channel messages. Debug builds serve it on 127.0.0.1:1430 when
//! `UBERMENSCH_WEB_BRIDGE=1`; the E2E server mounts the same router.

use std::convert::Infallible;

use axum::extract::{Path, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;

use super::EngineRef;
use crate::api;
use crate::model::ChannelMessage;

/// Serves the bridge on 127.0.0.1:1430 for the app's debug builds.
pub fn start(app: &AppHandle) {
    if !cfg!(debug_assertions) || std::env::var("UBERMENSCH_WEB_BRIDGE").as_deref() != Ok("1") {
        return;
    }
    let engine = app.state::<EngineRef>().inner().clone();
    tauri::async_runtime::spawn(async move {
        match tokio::net::TcpListener::bind("127.0.0.1:1430").await {
            Ok(listener) => {
                log::info!("web bridge on http://127.0.0.1:1430");
                if let Err(e) = axum::serve(listener, router(engine)).await {
                    log::warn!("web bridge stopped: {e}");
                }
            }
            Err(e) => log::warn!("web bridge: {e}"),
        }
    });
}

pub fn router(engine: EngineRef) -> Router {
    Router::new()
        .route("/events", get(events))
        .route("/invoke/{cmd}", post(invoke).options(preflight))
        .with_state(engine)
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

async fn events(State(engine): State<EngineRef>) -> Response {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ChannelMessage>();
    engine.writer.set_channel(move |m| {
        let _ = tx.send(m);
    });
    let stream = UnboundedReceiverStream::new(rx).map(|m| {
        Ok::<_, Infallible>(Event::default().data(serde_json::to_string(&m).unwrap_or_default()))
    });
    cors(Sse::new(stream).into_response())
}

fn arg<T: serde::de::DeserializeOwned>(v: &Value, k: &str) -> Result<T, String> {
    serde_json::from_value(v.get(k).cloned().unwrap_or(Value::Null))
        .map_err(|e| format!("{k}: {e}"))
}

/// Runs a command by name through [`crate::api`], as the Tauri commands do.
async fn invoke(
    State(engine): State<EngineRef>,
    Path(cmd): Path<String>,
    Json(args): Json<Value>,
) -> Response {
    let e = &engine;
    let result: Result<Value, String> = async {
        Ok(match cmd.as_str() {
            // Unlike the Tauri command, events arrive on `/events` and no window is shown.
            "connect" => json!(api::launch_info(e)?),
            "submit" => {
                api::submit(e, arg(&args, "actions")?);
                Value::Null
            }
            "fetch_rows" => json!(api::fetch_rows(
                e,
                arg(&args, "drafts")?,
                &arg::<Vec<String>>(&args, "keys")?,
            )?),
            "get_bodies" => json!(api::get_bodies(e, &arg::<Vec<_>>(&args, "ids")?)?),
            "autocomplete" => json!(api::autocomplete(
                e,
                arg(&args, "account")?,
                &arg::<String>(&args, "query")?,
            )?),
            "snooze_options" => json!(api::snooze_options(&arg::<String>(&args, "text")?)),
            "perf_report" => {
                api::perf_report(
                    &arg::<String>(&args, "action")?,
                    arg(&args, "ms")?,
                    arg(&args, "budget")?,
                );
                Value::Null
            }
            "add_account" => json!(api::add_account(e).await?),
            "sign_in_again" => {
                api::sign_in_again(e, arg(&args, "account")?).await?;
                Value::Null
            }
            "open_attachment" => {
                api::open_attachment(e, arg(&args, "attachment")?).await?;
                Value::Null
            }
            "quit_now" => Value::Null,
            #[cfg(feature = "fake")]
            "demo_deliver" => {
                super::demo::deliver(&arg::<String>(&args, "subject")?);
                e.kick_polls();
                Value::Null
            }
            "add_attachment" => json!(api::add_attachment(
                &arg::<String>(&args, "draft")?,
                &arg::<String>(&args, "filename")?,
                &arg::<String>(&args, "mimeType")?,
                &arg::<String>(&args, "data")?,
            )?),
            other => return Err(format!("unknown command {other}")),
        })
    }
    .await;
    cors(match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!(e))).into_response(),
    })
}
