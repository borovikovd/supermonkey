//! The web around the app: the built UI, the remote image host, the sink
//! for hostile destinations, and a look inside the database files and the
//! draft folders.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::server::{Ctl, Fail, Hit, Server};

/// The app's Tauri config, for the CSP the release app runs the UI under.
const TAURI_CONF: &str = include_str!("../../../../tauri.conf.json");

/// The release app's CSP (`app.security.csp`), so the suite runs the UI
/// under it (SPEC Testing). The page talks to this server instead of
/// Tauri's IPC, so `connect-src` also allows the server's own origin.
fn release_csp() -> Result<String, String> {
    let conf: Value = serde_json::from_str(TAURI_CONF).map_err(|e| e.to_string())?;
    let csp = conf
        .pointer("/app/security/csp")
        .and_then(Value::as_str)
        .ok_or("tauri.conf.json has no app.security.csp")?;
    let directives: Vec<String> = csp
        .split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| {
            if d.split_whitespace().next() == Some("connect-src") {
                format!("{d} 'self'")
            } else {
                d.to_owned()
            }
        })
        .collect();
    Ok(directives.join("; "))
}

/// A 1×1 PNG.
const PIXEL: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0,
    1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54,
    0x78, 0x9C, 0x63, 0x60, 0x60, 0xF8, 0x0F, 0, 0x01, 0x04, 0x01, 0x00, 0x5F, 0xB3, 0x7E, 0x3B, 0,
    0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn hit(s: &Server, method: &Method, uri: &Uri, body: &[u8]) {
    let at = s.now();
    s.hits.lock().push(Hit {
        method: method.to_string(),
        path: uri.path().to_owned(),
        body: String::from_utf8_lossy(body).into_owned(),
        at,
    });
}

/// The remote image host: any `/img/<name>` is a pixel, and every fetch is
/// logged.
pub(crate) async fn remote_image(
    State(s): State<Arc<Server>>,
    method: Method,
    uri: Uri,
) -> Response {
    hit(&s, &method, &uri, &[]);
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        PIXEL,
    )
        .into_response()
}

/// A sink for hostile destinations (`/sink/evil.test/...`): logs and
/// answers blank.
pub(crate) async fn sink(
    State(s): State<Arc<Server>>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    hit(&s, &method, &uri, &body);
    (
        [
            (header::CONTENT_TYPE, "text/html"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        "",
    )
        .into_response()
}

/// Fetches of the image host and the sink, oldest first.
pub(crate) async fn hits(State(s): State<Arc<Server>>) -> Ctl {
    Ok(Json(json!(&*s.hits.lock())))
}

#[derive(Deserialize)]
pub(crate) struct ContainsReq {
    text: String,
}

/// Whether the database files hold `text` anywhere (SPEC Security: no
/// tokens in SQLite).
pub(crate) async fn db_contains(State(s): State<Arc<Server>>, Query(q): Query<ContainsReq>) -> Ctl {
    let needle = q.text.as_bytes();
    if needle.is_empty() {
        return Err(Fail("empty text".into()));
    }
    let db = s.db_path();
    let mut found = false;
    for suffix in ["", "-wal", "-journal"] {
        let path = PathBuf::from(format!("{}{suffix}", db.display()));
        match tokio::fs::read(&path).await {
            Ok(bytes) => found |= bytes.windows(needle.len()).any(|w| w == needle),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(Json(json!({ "found": found })))
}

/// The drafts whose added files are in the data folder (`drafts/<draft>`),
/// sorted: SPEC Drafts says the folder goes with the draft, with nothing on
/// screen to show it.
pub(crate) async fn draft_folders(State(s): State<Arc<Server>>) -> Ctl {
    let mut names = Vec::new();
    match tokio::fs::read_dir(s.data_dir.join("drafts")).await {
        Ok(mut dir) => {
            while let Some(entry) = dir.next_entry().await? {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    names.sort();
    Ok(Json(json!(names)))
}

/// Serves the built UI; unknown paths get `index.html`.
pub(crate) async fn static_file(State(s): State<Arc<Server>>, uri: Uri) -> Response {
    let rel = uri.path().trim_start_matches('/');
    let safe = !rel.split('/').any(|segment| segment == "..");
    let mut path = s.dist.join(if rel.is_empty() || !safe {
        "index.html"
    } else {
        rel
    });
    if !path.is_file() {
        path = s.dist.join("index.html");
    }
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return (StatusCode::NOT_FOUND, "build the UI first: vp build").into_response();
    };
    let content_type = mime_for(&path);
    if path.extension().and_then(|e| e.to_str()) != Some("html") {
        return ([(header::CONTENT_TYPE, content_type)], bytes).into_response();
    }
    match release_csp() {
        Ok(csp) => (
            [
                (header::CONTENT_TYPE, content_type.to_owned()),
                (header::CONTENT_SECURITY_POLICY, csp),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

fn mime_for(path: &Path) -> &'static str {
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
