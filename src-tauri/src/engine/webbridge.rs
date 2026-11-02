//! Debug builds only: the same command surface over HTTP on 127.0.0.1:1430,
//! so the UI can run in a browser against the real Rust core for automated
//! checks. Started only when `UBERMENSCH_WEB_BRIDGE=1`.

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

use crate::Core;
use crate::commands;
use crate::model::{ChannelMessage, LaunchInfo};

pub fn start(app: AppHandle) {
    if !cfg!(debug_assertions) || std::env::var("UBERMENSCH_WEB_BRIDGE").as_deref() != Ok("1") {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let router = Router::new()
            .route("/events", get(events))
            .route("/invoke/{cmd}", post(invoke).options(preflight))
            .with_state(app);
        match tokio::net::TcpListener::bind("127.0.0.1:1430").await {
            Ok(listener) => {
                log::info!("web bridge on http://127.0.0.1:1430");
                if let Err(e) = axum::serve(listener, router).await {
                    log::warn!("web bridge stopped: {e}");
                }
            }
            Err(e) => log::warn!("web bridge: {e}"),
        }
    });
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

async fn events(State(app): State<AppHandle>) -> Response {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ChannelMessage>();
    app.state::<Core>().writer.set_channel(move |m| {
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

/// Runs a command by name with the same functions the Tauri commands use.
async fn invoke(
    State(app): State<AppHandle>,
    Path(cmd): Path<String>,
    Json(args): Json<Value>,
) -> Response {
    let result: Result<Value, String> = async {
        Ok(match cmd.as_str() {
            // Unlike the Tauri command, events arrive on `/events` and no window is shown.
            "connect" => {
                let last_action = app
                    .state::<Core>()
                    .reads
                    .with(crate::db::queries::last_action)
                    .map_err(|e| e.to_string())?;
                json!(LaunchInfo {
                    last_action,
                    debug: true
                })
            }
            "submit" => {
                commands::submit(app.state(), arg(&args, "actions")?);
                Value::Null
            }
            "fetch_rows" => json!(commands::fetch_rows(
                app.state(),
                arg(&args, "drafts")?,
                arg(&args, "keys")?,
            )?),
            "get_bodies" => json!(commands::get_bodies(app.state(), arg(&args, "ids")?)?),
            "autocomplete" => json!(commands::autocomplete(
                app.state(),
                arg(&args, "account")?,
                arg(&args, "query")?,
            )?),
            "snooze_options" => json!(commands::snooze_options(arg(&args, "text")?)),
            "perf_report" => {
                commands::perf_report(
                    arg(&args, "action")?,
                    arg(&args, "ms")?,
                    arg(&args, "budget")?,
                );
                Value::Null
            }
            "add_account" => json!(commands::add_account(app.clone()).await?),
            "sign_in_again" => {
                commands::sign_in_again(app.clone(), arg(&args, "account")?).await?;
                Value::Null
            }
            "open_attachment" => {
                commands::open_attachment(app.clone(), arg(&args, "attachment")?).await?;
                Value::Null
            }
            "quit_now" => Value::Null,
            #[cfg(feature = "fake")]
            "demo_deliver" => {
                crate::engine::demo::deliver(&arg::<String>(&args, "subject")?);
                for rt in app
                    .state::<crate::engine::EngineRef>()
                    .accounts
                    .lock()
                    .values()
                {
                    rt.poll_kick.notify_one();
                }
                Value::Null
            }
            "add_attachment" => json!(commands::add_attachment(
                arg(&args, "draft")?,
                arg(&args, "filename")?,
                arg(&args, "mimeType")?,
                arg(&args, "data")?,
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
