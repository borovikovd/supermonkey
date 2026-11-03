//! HTTP handlers: the Gmail REST routes under `/gmail/v1/users/{user}` and Google OAuth
//! (`/auth`, `/token`, `/revoke`).

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::PathRejection;
use axum::extract::{FromRequestParts, Path, RawQuery, State};
use axum::http::header::{AUTHORIZATION, LOCATION, RETRY_AFTER};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::mailbox::{Mailbox, Missing};
use crate::{AuthCode, Shared};

const BASE64URL_LENIENT: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

const GMAIL_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";

pub(crate) fn router(state: Arc<Shared>) -> Router {
    let gmail = Router::new()
        .route("/profile", get(profile))
        .route("/settings/sendAs", get(send_as))
        .route("/labels", get(list_labels).post(create_label))
        .route("/history", get(list_history))
        .route("/threads", get(list_threads))
        .route("/threads/{id}", get(get_thread))
        .route("/messages/batchModify", post(batch_modify))
        .route("/messages/{id}", get(get_message))
        .route("/messages/{id}/trash", post(trash_message))
        .route(
            "/messages/{id}/attachments/{attachment}",
            get(get_attachment),
        )
        .route("/drafts", get(list_drafts).post(create_draft))
        .route("/drafts/send", post(send_draft))
        .route(
            "/drafts/{id}",
            get(get_draft).put(update_draft).delete(delete_draft),
        );
    Router::new()
        .nest("/gmail/v1/users/{user}", gmail)
        .route("/auth", get(auth))
        .route("/token", post(token))
        .route("/revoke", post(revoke))
        .with_state(state)
}

/// A Google API error response: `{"error":{"code","message","errors":[{"reason"}],"status"}}`.
pub(crate) fn google_error(
    status: u16,
    reason: &str,
    message: &str,
    retry_after_secs: Option<u64>,
) -> Response {
    let status_name = match status {
        400 => "INVALID_ARGUMENT",
        401 => "UNAUTHENTICATED",
        403 => "PERMISSION_DENIED",
        404 => "NOT_FOUND",
        409 => "ALREADY_EXISTS",
        429 => "RESOURCE_EXHAUSTED",
        503 => "UNAVAILABLE",
        500..=599 => "INTERNAL",
        _ => "FAILED_PRECONDITION",
    };
    let body = json!({
        "error": {
            "code": status,
            "message": message,
            "errors": [{ "message": message, "domain": "global", "reason": reason }],
            "status": status_name,
        }
    });
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut response = (code, Json(body)).into_response();
    if let Some(secs) = retry_after_secs {
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from(secs));
    }
    response
}

/// An error a Gmail handler answers with.
enum ApiError {
    Unauthenticated,
    NotFound,
    BadRequest(String),
    DuplicateLabel,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Unauthenticated => google_error(
                401,
                "authError",
                "Request had invalid authentication credentials.",
                None,
            ),
            ApiError::NotFound => {
                google_error(404, "notFound", "Requested entity was not found.", None)
            }
            ApiError::BadRequest(message) => google_error(400, "invalidArgument", &message, None),
            ApiError::DuplicateLabel => {
                google_error(409, "duplicate", "Label name exists or conflicts", None)
            }
        }
    }
}

impl From<Missing> for ApiError {
    fn from(_: Missing) -> Self {
        ApiError::NotFound
    }
}

type ApiResult<T = Json<Value>> = Result<T, ApiError>;

/// The caller of a Gmail route: the `{user}` path segment and the bearer token. The token is
/// checked by [`Caller::mailbox`], not on extraction, so a malformed body answers 400 before
/// a bad token answers 401.
struct Caller {
    state: Arc<Shared>,
    user: String,
    token: String,
}

impl FromRequestParts<Arc<Shared>> for Caller {
    type Rejection = PathRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<Shared>,
    ) -> Result<Self, Self::Rejection> {
        let Path(mut params) =
            Path::<HashMap<String, String>>::from_request_parts(parts, state).await?;
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or_default();
        Ok(Caller {
            state: state.clone(),
            user: params.remove("user").unwrap_or_default(),
            token: token.to_owned(),
        })
    }
}

impl Caller {
    /// Checks the bearer token and runs `f` on the user's mailbox.
    fn mailbox<T>(&self, f: impl FnOnce(&mut Mailbox) -> ApiResult<T>) -> ApiResult<T> {
        let mut world = self.state.world();
        let email = match world.access_tokens.get(&self.token) {
            Some((email, expiry))
                if *expiry > Instant::now() && (self.user == "me" || self.user == *email) =>
            {
                email.clone()
            }
            _ => return Err(ApiError::Unauthenticated),
        };
        f(world.mailboxes.get_mut(&email).expect("token user exists"))
    }
}

/// Query parameters, keeping repeated keys.
struct Params(Vec<(String, String)>);

impl<S: Send + Sync> FromRequestParts<S> for Params {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let RawQuery(query) = RawQuery::from_request_parts(parts, state).await?;
        Ok(Params::parse(query.unwrap_or_default().as_bytes()))
    }
}

impl Params {
    /// Parses `application/x-www-form-urlencoded` pairs.
    fn parse(input: &[u8]) -> Params {
        Params(url::form_urlencoded::parse(input).into_owned().collect())
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .collect()
    }

    fn format(&self) -> &str {
        self.get("format").unwrap_or("full")
    }

    /// `(offset, page size)` from `pageToken` and `maxResults`.
    fn page(&self) -> (usize, usize) {
        let offset = self
            .get("pageToken")
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        let size = self
            .get("maxResults")
            .and_then(|m| m.parse().ok())
            .unwrap_or(100usize)
            .clamp(1, 500);
        (offset, size)
    }
}

/// Adds the page of `items` that `params` asks for to a list response: under `key`, left out
/// when the page is empty as Gmail does, plus `nextPageToken` while more pages follow.
fn with_page<T>(
    mut out: Value,
    key: &str,
    items: &[T],
    params: &Params,
    to_json: impl FnMut(&T) -> Value,
) -> Value {
    let (offset, size) = params.page();
    let page: Vec<Value> = items.iter().skip(offset).take(size).map(to_json).collect();
    if !page.is_empty() {
        out[key] = Value::Array(page);
    }
    if offset + size < items.len() {
        out["nextPageToken"] = json!((offset + size).to_string());
    }
    out
}

fn json_body(body: &Bytes) -> ApiResult<Value> {
    serde_json::from_slice(body)
        .map_err(|e| ApiError::BadRequest(format!("Invalid JSON payload received: {e}")))
}

fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

async fn profile(caller: Caller) -> ApiResult {
    caller.mailbox(|mb| Ok(Json(mb.profile_json())))
}

async fn send_as(caller: Caller) -> ApiResult {
    caller.mailbox(|mb| Ok(Json(mb.send_as_json())))
}

async fn list_labels(caller: Caller) -> ApiResult {
    caller.mailbox(|mb| Ok(Json(mb.labels_json())))
}

async fn create_label(caller: Caller, body: Bytes) -> ApiResult {
    let body = json_body(&body)?;
    caller.mailbox(|mb| {
        let name = body["name"]
            .as_str()
            .filter(|n| !n.trim().is_empty())
            .ok_or_else(|| ApiError::BadRequest("Invalid label name".into()))?;
        let id = mb.create_label(name).ok_or(ApiError::DuplicateLabel)?;
        Ok(Json(mb.label(&id).expect("just created").json()))
    })
}

async fn list_history(caller: Caller, params: Params) -> ApiResult {
    caller.mailbox(|mb| {
        let start: u64 = params
            .get("startHistoryId")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| ApiError::BadRequest("Missing startHistoryId".into()))?;
        let records = mb.history_after(start)?;
        let out = json!({ "historyId": mb.history_id.to_string() });
        Ok(Json(with_page(
            out,
            "history",
            &records,
            &params,
            Value::clone,
        )))
    })
}

async fn list_threads(caller: Caller, params: Params) -> ApiResult {
    let now = caller.state.now_ms();
    caller.mailbox(|mb| {
        let ids = mb.search_threads(
            params.get("q").unwrap_or_default(),
            &params.all("labelIds"),
            params.get("includeSpamTrash") == Some("true"),
            now,
        );
        let out = json!({ "resultSizeEstimate": ids.len() });
        Ok(Json(with_page(out, "threads", &ids, &params, |id| {
            mb.thread_summary_json(id)
        })))
    })
}

async fn get_thread(
    caller: Caller,
    Path((_, id)): Path<(String, String)>,
    params: Params,
) -> ApiResult {
    caller.mailbox(|mb| {
        mb.thread_json(&id, params.format(), &params.all("metadataHeaders"))
            .map(Json)
            .ok_or(ApiError::NotFound)
    })
}

async fn get_message(
    caller: Caller,
    Path((_, id)): Path<(String, String)>,
    params: Params,
) -> ApiResult {
    caller.mailbox(|mb| {
        let msg = mb.messages.get(&id).ok_or(ApiError::NotFound)?;
        Ok(Json(
            msg.json(params.format(), &params.all("metadataHeaders")),
        ))
    })
}

async fn get_attachment(
    caller: Caller,
    Path((_, id, attachment)): Path<(String, String, String)>,
) -> ApiResult {
    caller.mailbox(|mb| {
        let data = mb
            .messages
            .get(&id)
            .and_then(|m| m.attachments.get(&attachment))
            .ok_or(ApiError::NotFound)?;
        Ok(Json(
            json!({ "size": data.len(), "data": URL_SAFE_NO_PAD.encode(data) }),
        ))
    })
}

async fn batch_modify(caller: Caller, body: Bytes) -> ApiResult<StatusCode> {
    let body = json_body(&body)?;
    caller.mailbox(|mb| {
        let ids = string_list(&body["ids"]);
        let add = string_list(&body["addLabelIds"]);
        let remove = string_list(&body["removeLabelIds"]);
        if let Some(bad) = add.iter().chain(&remove).find(|l| mb.label(l).is_none()) {
            return Err(ApiError::BadRequest(format!("Invalid label: {bad}")));
        }
        if ids.is_empty() || ids.iter().any(|id| !mb.messages.contains_key(id)) {
            return Err(ApiError::NotFound);
        }
        for id in &ids {
            mb.modify(id, &add, &remove)?;
        }
        Ok(StatusCode::NO_CONTENT)
    })
}

async fn trash_message(caller: Caller, Path((_, id)): Path<(String, String)>) -> ApiResult {
    caller.mailbox(|mb| {
        mb.modify(&id, &["TRASH".into()], &["INBOX".into(), "SPAM".into()])?;
        Ok(Json(mb.messages[&id].json("minimal", &[])))
    })
}

fn draft_json(mb: &Mailbox, draft_id: &str, format: &str) -> Value {
    let msg = &mb.messages[&mb.drafts[draft_id]];
    let message = if format == "minimal" {
        json!({ "id": msg.id, "threadId": msg.thread_id, "labelIds": msg.label_ids })
    } else {
        msg.json(format, &[])
    };
    json!({ "id": draft_id, "message": message })
}

/// The raw bytes and thread id of a request's `message`.
fn draft_input(body: &Value, mb: &Mailbox) -> ApiResult<(Vec<u8>, Option<String>)> {
    let raw = body["message"]["raw"]
        .as_str()
        .ok_or_else(|| ApiError::BadRequest("Missing draft message".into()))?;
    let raw = BASE64URL_LENIENT
        .decode(raw.trim())
        .map_err(|e| ApiError::BadRequest(format!("Invalid value for ByteString: {e}")))?;
    let thread_id = body["message"]["threadId"].as_str().map(str::to_owned);
    if let Some(thread) = &thread_id
        && !mb.messages.values().any(|m| &m.thread_id == thread)
    {
        return Err(ApiError::NotFound);
    }
    Ok((raw, thread_id))
}

async fn list_drafts(caller: Caller, params: Params) -> ApiResult {
    caller.mailbox(|mb| {
        let ids: Vec<&String> = mb.drafts.keys().rev().collect();
        let out = json!({ "resultSizeEstimate": ids.len() });
        Ok(Json(with_page(out, "drafts", &ids, &params, |id| {
            let msg = &mb.messages[&mb.drafts[*id]];
            json!({ "id": id, "message": { "id": msg.id, "threadId": msg.thread_id } })
        })))
    })
}

async fn get_draft(
    caller: Caller,
    Path((_, id)): Path<(String, String)>,
    params: Params,
) -> ApiResult {
    caller.mailbox(|mb| {
        if !mb.drafts.contains_key(&id) {
            return Err(ApiError::NotFound);
        }
        Ok(Json(draft_json(mb, &id, params.format())))
    })
}

async fn create_draft(caller: Caller, body: Bytes) -> ApiResult {
    let body = json_body(&body)?;
    let now = caller.state.clock_ms();
    caller.mailbox(|mb| {
        let (raw, thread_id) = draft_input(&body, mb)?;
        let draft_id = mb.create_draft(raw, thread_id, now);
        Ok(Json(draft_json(mb, &draft_id, "minimal")))
    })
}

async fn update_draft(
    caller: Caller,
    Path((_, id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let body = json_body(&body)?;
    let now = caller.state.clock_ms();
    caller.mailbox(|mb| {
        if !mb.drafts.contains_key(&id) {
            return Err(ApiError::NotFound);
        }
        let (raw, thread_id) = draft_input(&body, mb)?;
        mb.update_draft(&id, raw, thread_id, now)?;
        Ok(Json(draft_json(mb, &id, "minimal")))
    })
}

/// Sends a draft: the draft and its message go away and a `SENT` message joins the thread.
async fn send_draft(caller: Caller, body: Bytes) -> ApiResult {
    let body = json_body(&body)?;
    let now = caller.state.clock_ms();
    caller.mailbox(|mb| {
        let id = body["id"]
            .as_str()
            .ok_or_else(|| ApiError::BadRequest("Missing draft id".into()))?;
        if !mb.drafts.contains_key(id) {
            return Err(ApiError::NotFound);
        }
        let raw = if body["message"]["raw"].is_string() {
            Some(draft_input(&body, mb)?.0)
        } else {
            None
        };
        let sent = mb.send_draft(id, raw, now)?;
        Ok(Json(mb.messages[&sent].json("minimal", &[])))
    })
}

async fn delete_draft(
    caller: Caller,
    Path((_, id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    caller.mailbox(|mb| {
        mb.delete_draft(&id)?;
        Ok(StatusCode::NO_CONTENT)
    })
}

fn oauth_error(error: &str, description: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": error, "error_description": description })),
    )
        .into_response()
}

fn invalid_grant() -> Response {
    oauth_error("invalid_grant", "Token has been expired or revoked.")
}

/// The consent page: checks the request and redirects to `redirect_uri` with a code.
async fn auth(
    State(s): State<Arc<Shared>>,
    params: Params,
) -> Result<Response, (StatusCode, &'static str)> {
    let bad_request = |message| (StatusCode::BAD_REQUEST, message);
    let mut world = s.world();
    world.last_auth_request = Some(params.0.clone());
    let (Some(redirect_uri), Some(challenge)) =
        (params.get("redirect_uri"), params.get("code_challenge"))
    else {
        return Err(bad_request("missing redirect_uri or code_challenge"));
    };
    if params.get("code_challenge_method") != Some("S256")
        || params.get("response_type") != Some("code")
    {
        return Err(bad_request("unsupported request"));
    }
    let email = match params.get("login_hint") {
        Some(hint) => hint.to_owned(),
        None => world
            .users
            .first()
            .cloned()
            .ok_or(bad_request("no users"))?,
    };
    let mut location = url::Url::parse(redirect_uri).expect("valid redirect_uri");
    if std::mem::take(&mut world.deny_next_auth) {
        location
            .query_pairs_mut()
            .append_pair("error", "access_denied")
            .append_pair("state", params.get("state").unwrap_or_default());
        return Ok((StatusCode::FOUND, [(LOCATION, location.to_string())]).into_response());
    }
    let code = format!("4/fake-code-{}", world.next());
    world.auth_codes.insert(
        code.clone(),
        AuthCode {
            email,
            redirect_uri: redirect_uri.to_owned(),
            code_challenge: challenge.to_owned(),
        },
    );
    location
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", params.get("state").unwrap_or_default())
        .append_pair("scope", params.get("scope").unwrap_or_default());
    Ok((StatusCode::FOUND, [(LOCATION, location.to_string())]).into_response())
}

/// The token endpoint: `authorization_code` (with PKCE) and `refresh_token` grants.
async fn token(State(s): State<Arc<Shared>>, body: Bytes) -> Response {
    let form = Params::parse(&body);
    let mut world = s.world();
    let (email, refresh) = match form.get("grant_type") {
        Some("refresh_token") => {
            let Some(email) = form
                .get("refresh_token")
                .and_then(|t| world.refresh_tokens.get(t))
                .cloned()
            else {
                return invalid_grant();
            };
            if world.invalid_grant.contains(&email) {
                return invalid_grant();
            }
            (email, None)
        }
        Some("authorization_code") => {
            let Some(code) = form.get("code").and_then(|c| world.auth_codes.remove(c)) else {
                return invalid_grant();
            };
            let verifier = form.get("code_verifier").unwrap_or_default();
            let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            if challenge != code.code_challenge
                || form.get("redirect_uri") != Some(&code.redirect_uri)
            {
                return invalid_grant();
            }
            let refresh = world.issue_refresh_token(&code.email);
            (code.email, Some(refresh))
        }
        _ => return oauth_error("unsupported_grant_type", "Invalid grant_type"),
    };
    let access = world.issue_access_token(&email);
    let mut out = json!({
        "access_token": access,
        "expires_in": world.token_lifetime.as_secs(),
        "scope": GMAIL_SCOPE,
        "token_type": "Bearer",
    });
    if let Some(refresh) = refresh {
        out["refresh_token"] = json!(refresh);
    }
    Json(out).into_response()
}

/// Revokes the grant behind a refresh or access token: every token of that user dies.
async fn revoke(State(s): State<Arc<Shared>>, mut params: Params, body: Bytes) -> Response {
    params
        .0
        .extend(url::form_urlencoded::parse(&body).into_owned());
    let mut world = s.world();
    let token = params.get("token").unwrap_or_default();
    let Some(email) = world.token_owner(token) else {
        return oauth_error("invalid_token", "Token expired or revoked");
    };
    world.revocations.push(crate::Revocation {
        token: token.to_owned(),
        email: email.clone(),
    });
    world.refresh_tokens.retain(|_, e| *e != email);
    world.access_tokens.retain(|_, (e, _)| *e != email);
    StatusCode::OK.into_response()
}
