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
use futures_util::Stream;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::Core;
use crate::model::*;

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
                let _ = axum::serve(listener, router).await;
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
    let stream: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> = Box::pin(
        UnboundedReceiverStream::new(rx)
            .map(|m| Ok(Event::default().data(serde_json::to_string(&m).unwrap_or_default()))),
    );
    cors(Sse::new(stream).into_response())
}

fn arg<T: serde::de::DeserializeOwned>(v: &Value, k: &str) -> Result<T, String> {
    serde_json::from_value(v.get(k).cloned().unwrap_or(Value::Null))
        .map_err(|e| format!("{k}: {e}"))
}

async fn invoke(
    State(app): State<AppHandle>,
    Path(cmd): Path<String>,
    Json(args): Json<Value>,
) -> Response {
    let core = app.state::<Core>();
    let result: Result<Value, String> = async {
        Ok(match cmd.as_str() {
            "connect" => {
                let last_action = core
                    .reads
                    .with(crate::db::queries::last_action)
                    .map_err(|e| e.to_string())?;
                json!(LaunchInfo {
                    last_action,
                    debug: true
                })
            }
            "submit" => {
                core.writer.actions(arg(&args, "actions")?);
                Value::Null
            }
            "fetch_rows" => {
                let drafts: bool = arg(&args, "drafts")?;
                let keys: Vec<String> = arg(&args, "keys")?;
                json!(
                    core.reads
                        .with(|c| crate::db::queries::rows_for_keys(c, drafts, &keys))
                        .map_err(|e| e.to_string())?
                )
            }
            "get_bodies" => {
                let ids: Vec<i64> = arg(&args, "ids")?;
                json!(
                    core.reads
                        .with(|c| crate::db::queries::bodies(c, &ids))
                        .map_err(|e| e.to_string())?
                )
            }
            "autocomplete" => {
                let account: i64 = arg(&args, "account")?;
                let query: String = arg(&args, "query")?;
                json!(
                    core.reads
                        .with(|c| crate::engine::contacts::suggest(c, account, &query, 6))
                        .map_err(|e| e.to_string())?
                )
            }
            "snooze_options" => json!(crate::commands::snooze_options(arg(&args, "text")?)),
            "perf_report" => {
                crate::commands::perf_report(
                    arg(&args, "action")?,
                    arg(&args, "ms")?,
                    arg(&args, "budget")?,
                );
                Value::Null
            }
            "add_account" => json!(
                crate::engine::auth::add_account(&app)
                    .await
                    .map_err(|e| e.to_string())?
            ),
            "sign_in_again" => {
                crate::engine::auth::sign_in_again(&app, arg(&args, "account")?)
                    .await
                    .map_err(|e| e.to_string())?;
                Value::Null
            }
            "open_attachment" => {
                crate::engine::attachments::open(&app, arg(&args, "attachment")?)
                    .await
                    .map_err(|e| e.to_string())?;
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
            "add_attachment" => json!(
                crate::commands::store_attachment(
                    &arg::<String>(&args, "draft")?,
                    &arg::<String>(&args, "filename")?,
                    &arg::<String>(&args, "mimeType")?,
                    &arg::<String>(&args, "data")?,
                )
                .map_err(|e| e.to_string())?
            ),
            other => return Err(format!("unknown command {other}")),
        })
    }
    .await;
    cors(match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!(e))).into_response(),
    })
}
