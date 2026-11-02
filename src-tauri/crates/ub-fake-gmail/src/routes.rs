//! HTTP handlers: the Gmail REST routes under `/gmail/v1/users/{user}` and Google OAuth
//! (`/auth`, `/token`, `/revoke`).

use std::sync::Arc;
use std::time::Instant;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::header::{AUTHORIZATION, LOCATION, RETRY_AFTER};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::mailbox::{Mailbox, label_json};
use crate::{AuthCode, Shared};

type AppState = Arc<Shared>;

const BASE64URL_LENIENT: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

pub(crate) fn router(state: AppState) -> Router {
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

/// An error a handler answers with.
pub(crate) struct ApiError(Box<Response>);

impl ApiError {
    fn new(response: Response) -> Self {
        ApiError(Box::new(response))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        *self.0
    }
}

fn not_found() -> ApiError {
    ApiError::new(google_error(
        404,
        "notFound",
        "Requested entity was not found.",
        None,
    ))
}

fn bad_request(message: &str) -> ApiError {
    ApiError::new(google_error(400, "invalidArgument", message, None))
}

type ApiResult<T = Json<Value>> = Result<T, ApiError>;

/// Checks the bearer token and runs `f` on the user's mailbox.
fn with_mailbox<T>(
    state: &AppState,
    headers: &HeaderMap,
    user: &str,
    f: impl FnOnce(&mut Mailbox) -> ApiResult<T>,
) -> ApiResult<T> {
    let mut world = state.world();
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let email = match world.access_tokens.get(token) {
        Some((email, expiry)) if *expiry > Instant::now() && (user == "me" || user == email) => {
            email.clone()
        }
        _ => {
            return Err(ApiError::new(google_error(
                401,
                "authError",
                "Request had invalid authentication credentials.",
                None,
            )));
        }
    };
    f(world.mailboxes.get_mut(&email).expect("token user exists"))
}

/// Query parameters, keeping repeated keys.
struct Params(Vec<(String, String)>);

impl Params {
    fn new(raw: Option<String>) -> Params {
        Params(
            url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes())
                .into_owned()
                .collect(),
        )
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

/// One page of `items` and the next page token.
fn paginate<T: Clone>(items: &[T], (offset, size): (usize, usize)) -> (Vec<T>, Option<String>) {
    let page = items.iter().skip(offset).take(size).cloned().collect();
    let next = (offset + size < items.len()).then(|| (offset + size).to_string());
    (page, next)
}

fn json_body(body: &Bytes) -> ApiResult<Value> {
    serde_json::from_slice(body).map_err(|_| bad_request("Invalid JSON payload received."))
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

async fn profile(
    State(s): State<AppState>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> ApiResult {
    with_mailbox(&s, &headers, &user, |mb| Ok(Json(mb.profile_json())))
}

async fn send_as(
    State(s): State<AppState>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> ApiResult {
    with_mailbox(&s, &headers, &user, |mb| Ok(Json(mb.send_as_json())))
}

async fn list_labels(
    State(s): State<AppState>,
    Path(user): Path<String>,
    headers: HeaderMap,
) -> ApiResult {
    with_mailbox(&s, &headers, &user, |mb| Ok(Json(mb.labels_json())))
}

async fn create_label(
    State(s): State<AppState>,
    Path(user): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let body = json_body(&body)?;
    with_mailbox(&s, &headers, &user, |mb| {
        let name = body["name"].as_str().filter(|n| !n.trim().is_empty());
        let name = name.ok_or_else(|| bad_request("Invalid label name"))?;
        let id = mb.create_label(name).ok_or_else(|| {
            ApiError::new(google_error(
                409,
                "duplicate",
                "Label name exists or conflicts",
                None,
            ))
        })?;
        Ok(Json(label_json(mb.label(&id).expect("just created"))))
    })
}

async fn list_history(
    State(s): State<AppState>,
    Path(user): Path<String>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> ApiResult {
    let params = Params::new(query);
    with_mailbox(&s, &headers, &user, |mb| {
        let start: u64 = params
            .get("startHistoryId")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| bad_request("Missing startHistoryId"))?;
        let records = mb.history_after(start).map_err(|_| not_found())?;
        let (page, next) = paginate(&records, params.page());
        let mut out = json!({ "historyId": mb.history_id.to_string() });
        if !page.is_empty() {
            out["history"] = json!(page);
        }
        if let Some(next) = next {
            out["nextPageToken"] = json!(next);
        }
        Ok(Json(out))
    })
}

async fn list_threads(
    State(s): State<AppState>,
    Path(user): Path<String>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> ApiResult {
    let params = Params::new(query);
    with_mailbox(&s, &headers, &user, |mb| {
        let ids = mb.search_threads(
            params.get("q").unwrap_or_default(),
            &params.all("labelIds"),
            params.get("includeSpamTrash") == Some("true"),
        );
        let (page, next) = paginate(&ids, params.page());
        let mut out = json!({ "resultSizeEstimate": ids.len() });
        if !page.is_empty() {
            out["threads"] = page.iter().map(|id| mb.thread_summary_json(id)).collect();
        }
        if let Some(next) = next {
            out["nextPageToken"] = json!(next);
        }
        Ok(Json(out))
    })
}

async fn get_thread(
    State(s): State<AppState>,
    Path((user, id)): Path<(String, String)>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> ApiResult {
    let params = Params::new(query);
    with_mailbox(&s, &headers, &user, |mb| {
        mb.thread_json(&id, params.format(), &params.all("metadataHeaders"))
            .map(Json)
            .ok_or_else(not_found)
    })
}

async fn get_message(
    State(s): State<AppState>,
    Path((user, id)): Path<(String, String)>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> ApiResult {
    let params = Params::new(query);
    with_mailbox(&s, &headers, &user, |mb| {
        let msg = mb.messages.get(&id).ok_or_else(not_found)?;
        Ok(Json(mb.message_json(
            msg,
            params.format(),
            &params.all("metadataHeaders"),
        )))
    })
}

async fn get_attachment(
    State(s): State<AppState>,
    Path((user, id, attachment)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> ApiResult {
    with_mailbox(&s, &headers, &user, |mb| {
        let data = mb
            .messages
            .get(&id)
            .and_then(|m| m.attachments.get(&attachment))
            .ok_or_else(not_found)?;
        Ok(Json(
            json!({ "size": data.len(), "data": URL_SAFE_NO_PAD.encode(data) }),
        ))
    })
}

async fn batch_modify(
    State(s): State<AppState>,
    Path(user): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let body = json_body(&body)?;
    with_mailbox(&s, &headers, &user, |mb| {
        let ids = string_list(&body["ids"]);
        let add = string_list(&body["addLabelIds"]);
        let remove = string_list(&body["removeLabelIds"]);
        if let Some(bad) = add.iter().chain(&remove).find(|l| mb.label(l).is_none()) {
            return Err(bad_request(&format!("Invalid label: {bad}")));
        }
        if ids.is_empty() || ids.iter().any(|id| !mb.messages.contains_key(id)) {
            return Err(not_found());
        }
        for id in &ids {
            let _ = mb.modify(id, &add, &remove);
        }
        Ok(StatusCode::NO_CONTENT)
    })
}

async fn trash_message(
    State(s): State<AppState>,
    Path((user, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult {
    with_mailbox(&s, &headers, &user, |mb| {
        mb.modify(&id, &["TRASH".into()], &["INBOX".into(), "SPAM".into()])
            .map_err(|_| not_found())?;
        Ok(Json(mb.message_json(&mb.messages[&id], "minimal", &[])))
    })
}

fn draft_json(mb: &Mailbox, draft_id: &str, format: &str) -> Value {
    let msg = &mb.messages[&mb.drafts[draft_id]];
    let message = if format == "minimal" {
        json!({ "id": msg.id, "threadId": msg.thread_id, "labelIds": msg.label_ids })
    } else {
        mb.message_json(msg, format, &[])
    };
    json!({ "id": draft_id, "message": message })
}

/// The raw bytes and thread id of a request's `message`.
fn draft_input(body: &Value, mb: &Mailbox) -> ApiResult<(Vec<u8>, Option<String>)> {
    let raw = body["message"]["raw"]
        .as_str()
        .ok_or_else(|| bad_request("Missing draft message"))?;
    let raw = BASE64URL_LENIENT
        .decode(raw.trim())
        .map_err(|_| bad_request("Invalid value for ByteString"))?;
    let thread_id = body["message"]["threadId"].as_str().map(str::to_owned);
    if let Some(thread) = &thread_id
        && !mb.messages.values().any(|m| &m.thread_id == thread)
    {
        return Err(not_found());
    }
    Ok((raw, thread_id))
}

async fn list_drafts(
    State(s): State<AppState>,
    Path(user): Path<String>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> ApiResult {
    let params = Params::new(query);
    with_mailbox(&s, &headers, &user, |mb| {
        let ids: Vec<String> = mb.drafts.keys().rev().cloned().collect();
        let (page, next) = paginate(&ids, params.page());
        let mut out = json!({ "resultSizeEstimate": ids.len() });
        if !page.is_empty() {
            out["drafts"] = page
                .iter()
                .map(|id| {
                    let msg = &mb.messages[&mb.drafts[id]];
                    json!({ "id": id, "message": { "id": msg.id, "threadId": msg.thread_id } })
                })
                .collect();
        }
        if let Some(next) = next {
            out["nextPageToken"] = json!(next);
        }
        Ok(Json(out))
    })
}

async fn get_draft(
    State(s): State<AppState>,
    Path((user, id)): Path<(String, String)>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> ApiResult {
    let params = Params::new(query);
    with_mailbox(&s, &headers, &user, |mb| {
        if !mb.drafts.contains_key(&id) {
            return Err(not_found());
        }
        Ok(Json(draft_json(mb, &id, params.format())))
    })
}

async fn create_draft(
    State(s): State<AppState>,
    Path(user): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let body = json_body(&body)?;
    with_mailbox(&s, &headers, &user, |mb| {
        let (raw, thread_id) = draft_input(&body, mb)?;
        let message_id = mb.insert_raw(raw, thread_id, vec!["DRAFT".into()], None);
        let draft_id = format!("r-{}", mb.new_id());
        mb.drafts.insert(draft_id.clone(), message_id);
        Ok(Json(draft_json(mb, &draft_id, "minimal")))
    })
}

async fn update_draft(
    State(s): State<AppState>,
    Path((user, id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let body = json_body(&body)?;
    with_mailbox(&s, &headers, &user, |mb| {
        let old = mb.drafts.get(&id).cloned().ok_or_else(not_found)?;
        let (raw, thread_id) = draft_input(&body, mb)?;
        let thread_id = thread_id.unwrap_or_else(|| mb.messages[&old].thread_id.clone());
        let _ = mb.delete(&old);
        let message_id = mb.insert_raw(raw, Some(thread_id), vec!["DRAFT".into()], None);
        mb.drafts.insert(id.clone(), message_id);
        Ok(Json(draft_json(mb, &id, "minimal")))
    })
}

/// Sends a draft: the draft and its message go away and a `SENT` message joins the thread.
async fn send_draft(
    State(s): State<AppState>,
    Path(user): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let body = json_body(&body)?;
    with_mailbox(&s, &headers, &user, |mb| {
        let id = body["id"]
            .as_str()
            .ok_or_else(|| bad_request("Missing draft id"))?;
        let old = mb.drafts.get(id).cloned().ok_or_else(not_found)?;
        let raw = if body["message"]["raw"].is_string() {
            draft_input(&body, mb)?.0
        } else {
            mb.messages[&old].raw.clone()
        };
        let thread_id = mb.messages[&old].thread_id.clone();
        mb.drafts.remove(id);
        let _ = mb.delete(&old);
        let sent = mb.insert_raw(raw, Some(thread_id), vec!["SENT".into()], None);
        Ok(Json(mb.message_json(&mb.messages[&sent], "minimal", &[])))
    })
}

async fn delete_draft(
    State(s): State<AppState>,
    Path((user, id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    with_mailbox(&s, &headers, &user, |mb| {
        let message = mb.drafts.remove(&id).ok_or_else(not_found)?;
        let _ = mb.delete(&message);
        Ok(StatusCode::NO_CONTENT)
    })
}

fn oauth_error(status: StatusCode, error: &str, description: &str) -> Response {
    (
        status,
        Json(json!({ "error": error, "error_description": description })),
    )
        .into_response()
}

/// The consent page: checks the request and redirects to `redirect_uri` with a code.
async fn auth(State(s): State<AppState>, RawQuery(query): RawQuery) -> Response {
    let params = Params::new(query);
    let mut world = s.world();
    world.last_auth_request = Some(params.0.clone());
    let (Some(redirect_uri), Some(challenge)) =
        (params.get("redirect_uri"), params.get("code_challenge"))
    else {
        return (
            StatusCode::BAD_REQUEST,
            "missing redirect_uri or code_challenge",
        )
            .into_response();
    };
    if params.get("code_challenge_method") != Some("S256")
        || params.get("response_type") != Some("code")
    {
        return (StatusCode::BAD_REQUEST, "unsupported request").into_response();
    }
    let email = match params.get("login_hint") {
        Some(hint) => hint.to_owned(),
        None => match world.users.first() {
            Some(first) => first.clone(),
            None => return (StatusCode::BAD_REQUEST, "no users").into_response(),
        },
    };
    let code = format!("4/fake-code-{}", world.next());
    world.auth_codes.insert(
        code.clone(),
        AuthCode {
            email,
            redirect_uri: redirect_uri.to_owned(),
            code_challenge: challenge.to_owned(),
        },
    );
    let mut location = url::Url::parse(redirect_uri).expect("valid redirect_uri");
    location
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", params.get("state").unwrap_or_default())
        .append_pair("scope", params.get("scope").unwrap_or_default());
    (StatusCode::FOUND, [(LOCATION, location.to_string())]).into_response()
}

/// The token endpoint: `authorization_code` (with PKCE) and `refresh_token` grants.
async fn token(State(s): State<AppState>, body: Bytes) -> Response {
    let form = Params(url::form_urlencoded::parse(&body).into_owned().collect());
    let mut world = s.world();
    let invalid_grant = || {
        oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "Token has been expired or revoked.",
        )
    };
    let lifetime = world.token_lifetime.as_secs();
    match form.get("grant_type") {
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
            let access = world.issue_access_token(&email);
            Json(json!({
                "access_token": access,
                "expires_in": lifetime,
                "scope": "https://www.googleapis.com/auth/gmail.modify",
                "token_type": "Bearer",
            }))
            .into_response()
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
            let access = world.issue_access_token(&code.email);
            Json(json!({
                "access_token": access,
                "refresh_token": refresh,
                "expires_in": lifetime,
                "scope": "https://www.googleapis.com/auth/gmail.modify",
                "token_type": "Bearer",
            }))
            .into_response()
        }
        _ => oauth_error(
            StatusCode::BAD_REQUEST,
            "unsupported_grant_type",
            "Invalid grant_type",
        ),
    }
}

/// Revokes the grant behind a refresh or access token: every token of that user dies.
async fn revoke(State(s): State<AppState>, RawQuery(query): RawQuery, body: Bytes) -> Response {
    let mut params = Params::new(query);
    params
        .0
        .extend(url::form_urlencoded::parse(&body).into_owned());
    let mut world = s.world();
    let token = params.get("token").unwrap_or_default();
    let email = world
        .refresh_tokens
        .get(token)
        .or_else(|| world.access_tokens.get(token).map(|(email, _)| email))
        .cloned();
    let Some(email) = email else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_token",
            "Token expired or revoked",
        );
    };
    world.refresh_tokens.retain(|_, e| *e != email);
    world.access_tokens.retain(|_, (e, _)| *e != email);
    StatusCode::OK.into_response()
}
