use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sm_fake_gmail::{Answer, FakeMessage, Fault, Matcher, NewAttachment, NewMessage};

use crate::gmail::RefreshToken;
use crate::store::{Priority as StorePriority, StoreError, with_write_tx};

use super::server::{ServerState, SignInDirective};

pub fn router(state: Arc<ServerState>) -> Router {
    Router::new()
        .route("/__test/account", post(add_account))
        .route("/__test/user", post(add_user))
        .route("/__test/quota", post(set_quota))
        .route("/__test/start", post(start_core))
        .route("/__test/stop", post(stop_core))
        .route("/__test/idle", get(wait_idle))
        .route("/__test/deliver", post(deliver_one))
        .route("/__test/deliver-batch", post(deliver_batch))
        .route("/__test/gmail/{email}", get(get_gmail))
        .route("/__test/gmail/{email}/modify", post(modify_message))
        .route("/__test/gmail/{email}/delete", post(delete_message))
        .route("/__test/gmail/{email}/delete-thread", post(delete_thread))
        .route("/__test/gmail/{email}/label", post(create_label))
        .route("/__test/gmail/{email}/send-as", post(add_send_as))
        .route("/__test/gmail/{email}/drafts", post(create_draft))
        .route("/__test/gmail/{email}/drafts/{id}", post(update_draft))
        .route(
            "/__test/gmail/{email}/drafts/{id}/delete",
            post(delete_draft),
        )
        .route("/__test/gmail/{email}/drafts/{id}/send", post(send_draft))
        .route(
            "/__test/gmail/{email}/expire-tokens",
            post(expire_access_tokens),
        )
        .route("/__test/gmail/{email}/expire-history", post(expire_history))
        .route("/__test/gmail/{email}/filters", get(get_filters))
        .route("/__test/invalid-grant", post(set_invalid_grant))
        .route("/__test/sign-in", post(set_next_sign_in))
        .route("/__test/tokens", get(list_tokens))
        .route("/__test/tokens/mode", get(token_file_mode))
        .route("/__test/tokens/forget", post(forget_token))
        .route("/__test/settings-scope", post(set_settings_scope))
        .route("/__test/revocations", get(list_revocations))
        .route("/__test/fault", post(add_fault))
        .route("/__test/release", post(release_fault))
        .route("/__test/clear-faults", post(clear_faults))
        .route("/__test/offline", post(set_offline))
        .route("/__test/requests", get(list_requests))
        .route("/__test/poll", post(trigger_poll))
        .route("/__test/clock", get(get_clock).post(move_clock))
        .route("/__test/focus", post(set_focus))
        .route("/__test/platform", get(get_platform))
        .route("/__test/clear-recorded", post(clear_recorded))
        .route("/__test/hits", get(list_hits))
        .route("/__test/db-contains", get(db_contains))
        .route(
            "/__test/draft-folders",
            get(list_draft_folders).post(create_draft_folder),
        )
        .route("/__test/draft-folders/block", post(block_draft_folders))
        .with_state(state)
}

fn err_resp(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (status, Json(json!({ "error": msg.to_string() }))).into_response()
}

#[derive(Deserialize)]
struct AccountBody {
    email: String,
    name: Option<String>,
}

async fn add_account(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<AccountBody>,
) -> Response {
    let user = state.ensure_user(&body.email, body.name.as_deref());
    if let Err(err) = state
        .tokens()
        .store(&body.email, &RefreshToken::new(user.refresh_token()))
    {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, err);
    }
    let display_name = body.name.unwrap_or_else(|| {
        body.email
            .split('@')
            .next()
            .unwrap_or(&body.email)
            .to_owned()
    });
    let email = body.email;

    if let Some(session) = state.bridge.session() {
        let email_c = email.clone();
        let res = session
            .core()
            .writer()
            .call(StorePriority::Ui, move |tx, _st| {
                let rec = match tx.read().account_by_email(&email_c)? {
                    Some(existing) => existing,
                    None => tx.create_account(&email_c, &display_name)?,
                };
                Ok::<_, StoreError>(rec.id.0)
            })
            .await;
        return match res {
            Ok(id) => Json(json!({ "id": id })).into_response(),
            Err(err) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, err),
        };
    }

    let db_path = state.data_dir.join("mail.db");
    let res = with_write_tx(&db_path, |wtx| {
        let rec = match wtx.read().account_by_email(&email)? {
            Some(existing) => existing,
            None => wtx.create_account(&email, &display_name)?,
        };
        Ok(rec.id.0)
    });
    match res {
        Ok(id) => Json(json!({ "id": id })).into_response(),
        Err(err) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, err),
    }
}

async fn add_user(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<AccountBody>,
) -> Response {
    let _ = state.ensure_user(&body.email, body.name.as_deref());
    Json(Value::Null).into_response()
}

#[derive(Deserialize)]
struct QuotaBody {
    production: bool,
}

async fn set_quota(State(state): State<Arc<ServerState>>, Json(body): Json<QuotaBody>) -> Response {
    state
        .quota_production
        .store(body.production, Ordering::Release);
    Json(Value::Null).into_response()
}

async fn start_core(State(state): State<Arc<ServerState>>) -> Response {
    match state.start_core().await {
        Ok(()) => Json(Value::Null).into_response(),
        Err(err) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, err),
    }
}

#[derive(Deserialize)]
struct StopBody {
    graceful: bool,
}

async fn stop_core(State(state): State<Arc<ServerState>>, Json(body): Json<StopBody>) -> Response {
    if let Some(session) = state.bridge.session() {
        if body.graceful {
            session.request_quit();
            session.wait_exited().await;
        } else {
            session.terminate().await;
        }
        state.bridge.set_session(None);
    }
    Json(Value::Null).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdleQuery {
    timeout_ms: Option<u64>,
}

async fn wait_idle(State(state): State<Arc<ServerState>>, Query(q): Query<IdleQuery>) -> Response {
    let timeout = Duration::from_millis(q.timeout_ms.unwrap_or(10_000));
    let (rev, ack, sign_ins, held) = state.wait_idle(timeout).await;
    Json(json!({
        "rev": rev,
        "ack": ack,
        "signIns": sign_ins,
        "held": held,
    }))
    .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentSpec {
    filename: String,
    mime_type: Option<String>,
    content: String,
    #[serde(default)]
    base64: bool,
    content_id: Option<String>,
    #[serde(default)]
    inline: bool,
    #[serde(default)]
    served_inline: bool,
}

impl AttachmentSpec {
    fn into_fake(self) -> NewAttachment {
        let data = if self.base64 {
            STANDARD
                .decode(self.content.trim())
                .unwrap_or_else(|_| self.content.into_bytes())
        } else {
            self.content.into_bytes()
        };
        NewAttachment {
            filename: self.filename,
            mime_type: self
                .mime_type
                .unwrap_or_else(|| "application/octet-stream".to_owned()),
            data,
            content_id: self.content_id,
            inline: self.inline,
            served_inline: self.served_inline,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryBody {
    account: String,
    from: String,
    to: Option<Vec<String>>,
    #[serde(default)]
    cc: Vec<String>,
    subject: String,
    text: Option<String>,
    html: Option<String>,
    charset: Option<String>,
    labels: Option<Vec<String>>,
    thread_id: Option<String>,
    in_reply_to: Option<String>,
    references: Option<String>,
    list_id: Option<String>,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    attachments: Vec<AttachmentSpec>,
    date: Option<u64>,
    after: Option<usize>,
    poll: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeliveredOut {
    id: String,
    thread_id: String,
    message_id: String,
}

fn deliver_msg(
    state: &ServerState,
    d: DeliveryBody,
    prior: &[DeliveredOut],
) -> Result<(DeliveredOut, Option<bool>), String> {
    let user = state
        .user(&d.account)
        .ok_or_else(|| format!("unknown account {}", d.account))?;
    let (thread_id, in_reply_to) = if let Some(idx) = d.after
        && let Some(prev) = prior.get(idx)
    {
        (
            d.thread_id.or_else(|| Some(prev.thread_id.clone())),
            d.in_reply_to.or_else(|| Some(prev.message_id.clone())),
        )
    } else {
        (d.thread_id, d.in_reply_to)
    };
    let labels = d
        .labels
        .unwrap_or_else(|| vec!["INBOX".to_owned(), "UNREAD".to_owned()]);
    let to = d.to.unwrap_or_else(|| vec![d.account.clone()]);
    let added = user.add_message(NewMessage {
        from: d.from,
        to,
        cc: d.cc,
        subject: d.subject,
        text: d.text,
        html: d.html,
        charset: d.charset,
        labels,
        thread_id,
        message_id: None,
        in_reply_to,
        references: d.references,
        list_id: d.list_id,
        extra_headers: d.headers,
        attachments: d
            .attachments
            .into_iter()
            .map(AttachmentSpec::into_fake)
            .collect(),
        internal_date: d.date,
    });
    Ok((
        DeliveredOut {
            id: added.id,
            thread_id: added.thread_id,
            message_id: added.rfc822_message_id,
        },
        d.poll,
    ))
}

async fn deliver_one(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<DeliveryBody>,
) -> Response {
    match deliver_msg(&state, body, &[]) {
        Ok((out, poll)) => {
            if poll.unwrap_or(true)
                && let Some(session) = state.bridge.session()
            {
                session.core().poll_all();
            }
            Json(out).into_response()
        }
        Err(err) => err_resp(StatusCode::NOT_FOUND, err),
    }
}

async fn deliver_batch(
    State(state): State<Arc<ServerState>>,
    Json(list): Json<Vec<DeliveryBody>>,
) -> Response {
    let mut out = Vec::with_capacity(list.len());
    let mut should_poll = false;
    for item in list {
        match deliver_msg(&state, item, &out) {
            Ok((delivered, poll)) => {
                if poll.unwrap_or(true) {
                    should_poll = true;
                }
                out.push(delivered);
            }
            Err(err) => return err_resp(StatusCode::NOT_FOUND, err),
        }
    }
    if should_poll && let Some(session) = state.bridge.session() {
        session.core().poll_all();
    }
    Json(out).into_response()
}

fn format_fake_message(m: FakeMessage) -> Value {
    let mut headers: HashMap<String, String> = HashMap::new();
    for (k, v) in m.headers {
        headers.insert(k.to_ascii_lowercase(), v);
    }
    json!({
        "id": m.id,
        "threadId": m.thread_id,
        "labels": m.label_ids,
        "date": m.internal_date,
        "headers": headers,
        "raw": String::from_utf8_lossy(&m.raw),
    })
}

async fn get_gmail(
    State(state): State<Arc<ServerState>>,
    AxumPath(email): AxumPath<String>,
) -> Response {
    let Some(user) = state.user(&email) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown account {email}"));
    };
    let messages: Vec<Value> = user
        .messages()
        .into_iter()
        .map(format_fake_message)
        .collect();
    let drafts: Vec<Value> = user
        .drafts()
        .into_iter()
        .map(|d| json!({ "id": d.id, "message": format_fake_message(d.message) }))
        .collect();
    let labels: Vec<Value> = user
        .labels()
        .into_iter()
        .map(|(id, name)| json!({ "id": id, "name": name }))
        .collect();
    let send_as: Vec<Value> = user
        .send_as()
        .into_iter()
        .map(|(addr, display_name, primary)| {
            json!({ "email": addr, "displayName": display_name, "primary": primary })
        })
        .collect();
    Json(json!({
        "messages": messages,
        "drafts": drafts,
        "labels": labels,
        "sendAs": send_as,
    }))
    .into_response()
}

#[derive(Deserialize)]
struct ModifyBody {
    id: String,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(default)]
    poll: bool,
}

async fn modify_message(
    State(state): State<Arc<ServerState>>,
    AxumPath(email): AxumPath<String>,
    Json(body): Json<ModifyBody>,
) -> Response {
    let Some(user) = state.user(&email) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown account {email}"));
    };
    let _changed = user.modify_labels(&body.id, &body.add, &body.remove);
    if body.poll
        && let Some(session) = state.bridge.session()
    {
        session.core().poll_all();
    }
    Json(Value::Null).into_response()
}

#[derive(Deserialize)]
struct IdPollBody {
    id: String,
    #[serde(default)]
    poll: bool,
}

async fn delete_message(
    State(state): State<Arc<ServerState>>,
    AxumPath(email): AxumPath<String>,
    Json(body): Json<IdPollBody>,
) -> Response {
    let Some(user) = state.user(&email) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown account {email}"));
    };
    let _deleted = user.delete_message(&body.id);
    if body.poll
        && let Some(session) = state.bridge.session()
    {
        session.core().poll_all();
    }
    Json(Value::Null).into_response()
}

async fn delete_thread(
    State(state): State<Arc<ServerState>>,
    AxumPath(email): AxumPath<String>,
    Json(body): Json<IdPollBody>,
) -> Response {
    let Some(user) = state.user(&email) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown account {email}"));
    };
    user.delete_thread(&body.id);
    if body.poll
        && let Some(session) = state.bridge.session()
    {
        session.core().poll_all();
    }
    Json(Value::Null).into_response()
}

#[derive(Deserialize)]
struct LabelBody {
    name: String,
}

async fn create_label(
    State(state): State<Arc<ServerState>>,
    AxumPath(email): AxumPath<String>,
    Json(body): Json<LabelBody>,
) -> Response {
    let Some(user) = state.user(&email) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown account {email}"));
    };
    match user.add_label(&body.name) {
        Some(id) => Json(json!({ "id": id })).into_response(),
        None => err_resp(StatusCode::CONFLICT, "label already exists"),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendAsBody {
    email: String,
    display_name: String,
}

async fn add_send_as(
    State(state): State<Arc<ServerState>>,
    AxumPath(email): AxumPath<String>,
    Json(body): Json<SendAsBody>,
) -> Response {
    let Some(user) = state.user(&email) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown account {email}"));
    };
    user.add_send_as(body.email, body.display_name);
    Json(Value::Null).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DraftSpecBody {
    from: Option<String>,
    #[serde(default)]
    to: Vec<String>,
    #[serde(default)]
    cc: Vec<String>,
    #[serde(default)]
    subject: String,
    text: Option<String>,
    html: Option<String>,
    thread_id: Option<String>,
    in_reply_to: Option<String>,
    references: Option<String>,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    attachments: Vec<AttachmentSpec>,
    date: Option<u64>,
    #[serde(default)]
    poll: bool,
}

impl DraftSpecBody {
    fn into_fake_message(self, default_from: &str) -> (NewMessage, bool) {
        let poll = self.poll;
        let msg = NewMessage {
            from: self.from.unwrap_or_else(|| default_from.to_owned()),
            to: self.to,
            cc: self.cc,
            subject: self.subject,
            text: self.text,
            html: self.html,
            charset: None,
            labels: vec!["DRAFT".to_owned()],
            thread_id: self.thread_id,
            message_id: None,
            in_reply_to: self.in_reply_to,
            references: self.references,
            list_id: None,
            extra_headers: self.headers,
            attachments: self
                .attachments
                .into_iter()
                .map(AttachmentSpec::into_fake)
                .collect(),
            internal_date: self.date,
        };
        (msg, poll)
    }
}

async fn create_draft(
    State(state): State<Arc<ServerState>>,
    AxumPath(email): AxumPath<String>,
    Json(body): Json<DraftSpecBody>,
) -> Response {
    let Some(user) = state.user(&email) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown account {email}"));
    };
    let (msg, poll) = body.into_fake_message(&email);
    let added = user.add_draft(msg);
    if poll && let Some(session) = state.bridge.session() {
        session.core().poll_all();
    }
    Json(json!({
        "id": added.id,
        "messageId": added.message_id,
        "threadId": added.thread_id,
        "rfc822MessageId": added.rfc822_message_id,
    }))
    .into_response()
}

async fn update_draft(
    State(state): State<Arc<ServerState>>,
    AxumPath((email, draft_id)): AxumPath<(String, String)>,
    Json(body): Json<DraftSpecBody>,
) -> Response {
    let Some(user) = state.user(&email) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown account {email}"));
    };
    let (msg, poll) = body.into_fake_message(&email);
    let Some(updated) = user.update_draft(&draft_id, msg) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown draft {draft_id}"));
    };
    if poll && let Some(session) = state.bridge.session() {
        session.core().poll_all();
    }
    Json(json!({
        "id": updated.id,
        "messageId": updated.message_id,
        "threadId": updated.thread_id,
        "rfc822MessageId": updated.rfc822_message_id,
    }))
    .into_response()
}

#[derive(Deserialize, Default)]
struct PollOpt {
    #[serde(default)]
    poll: bool,
}

async fn delete_draft(
    State(state): State<Arc<ServerState>>,
    AxumPath((email, draft_id)): AxumPath<(String, String)>,
    body: Option<Json<PollOpt>>,
) -> Response {
    let Some(user) = state.user(&email) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown account {email}"));
    };
    let _deleted = user.delete_draft(&draft_id);
    if body.is_some_and(|b| b.0.poll)
        && let Some(session) = state.bridge.session()
    {
        session.core().poll_all();
    }
    Json(Value::Null).into_response()
}

async fn send_draft(
    State(state): State<Arc<ServerState>>,
    AxumPath((email, draft_id)): AxumPath<(String, String)>,
    body: Option<Json<PollOpt>>,
) -> Response {
    let Some(user) = state.user(&email) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown account {email}"));
    };
    let Some(sent) = user.send_draft(&draft_id) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown draft {draft_id}"));
    };
    if body.is_some_and(|b| b.0.poll)
        && let Some(session) = state.bridge.session()
    {
        session.core().poll_all();
    }
    Json(DeliveredOut {
        id: sent.id,
        thread_id: sent.thread_id,
        message_id: sent.rfc822_message_id,
    })
    .into_response()
}

async fn expire_access_tokens(
    State(state): State<Arc<ServerState>>,
    AxumPath(email): AxumPath<String>,
) -> Response {
    if let Some(user) = state.user(&email) {
        user.expire_access_tokens();
    }
    Json(Value::Null).into_response()
}

async fn expire_history(
    State(state): State<Arc<ServerState>>,
    AxumPath(email): AxumPath<String>,
) -> Response {
    if let Some(user) = state.user(&email) {
        user.expire_history();
    }
    Json(Value::Null).into_response()
}

async fn get_filters(
    State(state): State<Arc<ServerState>>,
    AxumPath(email): AxumPath<String>,
) -> Response {
    let Some(user) = state.user(&email) else {
        return err_resp(StatusCode::NOT_FOUND, format!("unknown account {email}"));
    };
    let list: Vec<Value> = user
        .filters()
        .into_iter()
        .map(|f| {
            json!({
                "id": f.id,
                "from": f.from,
                "add": f.add,
                "remove": f.remove,
            })
        })
        .collect();
    Json(list).into_response()
}

#[derive(Deserialize)]
struct InvalidGrantBody {
    email: String,
    on: bool,
}

async fn set_invalid_grant(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<InvalidGrantBody>,
) -> Response {
    if let Some(user) = state.user(&body.email) {
        user.set_invalid_grant(body.on);
    }
    Json(Value::Null).into_response()
}

#[derive(Deserialize)]
struct SignInBody {
    email: Option<String>,
    #[serde(default)]
    abandon: bool,
    #[serde(default)]
    deny: bool,
}

async fn set_next_sign_in(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<SignInBody>,
) -> Response {
    *state.next_sign_in.lock() = Some(SignInDirective {
        email: body.email,
        abandon: body.abandon,
        deny: body.deny,
    });
    Json(Value::Null).into_response()
}

async fn list_tokens(State(state): State<Arc<ServerState>>) -> Response {
    let items: Vec<Value> = state
        .tokens()
        .entries()
        .unwrap_or_default()
        .into_iter()
        .map(|(account, tok)| json!({ "account": account, "token": tok.expose() }))
        .collect();
    Json(items).into_response()
}

async fn token_file_mode(State(state): State<Arc<ServerState>>) -> Response {
    #[cfg(unix)]
    let mode: Option<String> = {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(state.tokens().path())
            .ok()
            .map(|m| format!("{:o}", m.permissions().mode() & 0o777))
    };
    #[cfg(not(unix))]
    let mode: Option<String> = None;
    Json(mode).into_response()
}

#[derive(Deserialize)]
struct EmailBody {
    email: String,
}

async fn forget_token(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<EmailBody>,
) -> Response {
    crate::core::log_err("delete test token", state.tokens().delete(&body.email));
    Json(Value::Null).into_response()
}

#[derive(Deserialize)]
struct ScopeBody {
    email: String,
    granted: bool,
}

async fn set_settings_scope(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<ScopeBody>,
) -> Response {
    if let Some(user) = state.user(&body.email) {
        user.set_settings_scope(body.granted);
    }
    Json(Value::Null).into_response()
}

async fn list_revocations(State(state): State<Arc<ServerState>>) -> Response {
    let list: Vec<Value> = state
        .fake
        .revocations()
        .into_iter()
        .map(|r| json!({ "token": r.token, "account": r.email }))
        .collect();
    Json(list).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FaultBody {
    api: Option<String>,
    method: Option<String>,
    path: Option<String>,
    account: Option<String>,
    #[serde(default)]
    query: HashMap<String, String>,
    nth: Option<usize>,
    times: Option<usize>,
    kind: String,
    status: Option<u16>,
    reason: Option<String>,
    retry_after_secs: Option<u64>,
    #[serde(default)]
    threads: Vec<String>,
}

fn parse_fault(
    kind: &str,
    status: Option<u16>,
    reason: Option<String>,
    retry_after_secs: Option<u64>,
    threads: Vec<String>,
) -> Result<Fault, String> {
    match kind {
        "hold" => Ok(Fault::Hold),
        "holdAnswer" => Ok(Fault::HoldAnswer),
        "drop" => Ok(Fault::Drop),
        "noResponse" => Ok(Fault::NoResponse),
        "status" => Ok(Fault::Status {
            status: status.unwrap_or(500),
            reason: reason.unwrap_or_else(|| "internalError".to_owned()),
            retry_after_secs,
        }),
        "statusAfterApply" => Ok(Fault::StatusAfterApply {
            status: status.unwrap_or(500),
            reason: reason.unwrap_or_else(|| "internalError".to_owned()),
        }),
        "omitThreads" => Ok(Fault::OmitThreads(threads)),
        other => Err(format!("unknown fault kind: {other}")),
    }
}

async fn add_fault(State(state): State<Arc<ServerState>>, Json(body): Json<FaultBody>) -> Response {
    let fault = match parse_fault(
        &body.kind,
        body.status,
        body.reason,
        body.retry_after_secs,
        body.threads,
    ) {
        Ok(f) => f,
        Err(msg) => return err_resp(StatusCode::BAD_REQUEST, msg),
    };
    let mut matcher = Matcher::new(
        body.method.as_deref().unwrap_or("*"),
        body.path.as_deref().unwrap_or("*"),
    );
    if let Some(api) = &body.api {
        matcher = matcher.for_api(api);
    }
    if let Some(acct) = &body.account {
        matcher = matcher.account(acct);
    }
    for (k, v) in body.query {
        matcher = matcher.query(&k, &v);
    }
    if let Some(n) = body.nth {
        matcher = matcher.nth(n);
    }
    if let Some(t) = body.times {
        matcher = matcher.times(t);
    }
    let id = state.fake.inject(matcher, fault);
    Json(json!({ "id": id })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutcomeBody {
    kind: String,
    status: Option<u16>,
    reason: Option<String>,
    retry_after_secs: Option<u64>,
    #[serde(default)]
    threads: Vec<String>,
}

#[derive(Deserialize)]
struct ReleaseBody {
    id: u64,
    #[serde(default)]
    min: usize,
    outcome: Option<OutcomeBody>,
}

async fn release_fault(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<ReleaseBody>,
) -> Response {
    let then = match body.outcome {
        Some(o) => match parse_fault(&o.kind, o.status, o.reason, o.retry_after_secs, o.threads) {
            Ok(f) => Some(f),
            Err(msg) => return err_resp(StatusCode::BAD_REQUEST, msg),
        },
        None => None,
    };
    if body.min > 0 {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while state.fake.held_by(body.id) < body.min && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    let released = state.fake.release(body.id, then.as_ref());
    Json(json!({ "released": released })).into_response()
}

async fn clear_faults(State(state): State<Arc<ServerState>>) -> Response {
    state.fake.clear_faults();
    if let Some(session) = state.bridge.session() {
        session.core().wake_retrying();
    }
    Json(Value::Null).into_response()
}

#[derive(Deserialize)]
struct OfflineBody {
    on: bool,
    #[serde(default)]
    probe: bool,
}

async fn set_offline(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<OfflineBody>,
) -> Response {
    state.offline.store(body.on, Ordering::Release);
    state.fake.set_offline(body.on).await;
    if !body.probe
        && let Some(session) = state.bridge.session()
    {
        session.core().set_online(!body.on).await;
    }
    Json(Value::Null).into_response()
}

async fn list_requests(State(state): State<Arc<ServerState>>) -> Response {
    let list: Vec<Value> = state
        .fake
        .requests()
        .into_iter()
        .map(|r| {
            let params: HashMap<String, String> = r.info.query_pairs().into_iter().collect();
            let answer = match r.answer {
                Some(Answer::Status(code)) => Value::from(code),
                Some(Answer::Dropped) => Value::from("dropped"),
                Some(Answer::NoResponse) => Value::from("noResponse"),
                None => Value::Null,
            };
            json!({
                "seq": r.seq,
                "api": r.info.api,
                "method": r.info.method,
                "path": r.info.path,
                "params": params,
                "account": r.info.account,
                "body": r.body,
                "at": r.at_ms,
                "held": r.held,
                "answer": answer,
                "origin": r.origin,
                "userAgent": r.user_agent,
            })
        })
        .collect();
    Json(list).into_response()
}

async fn trigger_poll(State(state): State<Arc<ServerState>>) -> Response {
    if let Some(session) = state.bridge.session() {
        session.core().poll_all();
    }
    Json(Value::Null).into_response()
}

async fn get_clock(State(state): State<Arc<ServerState>>) -> Response {
    Json(json!({ "now": state.clock.now().0 })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClockBody {
    advance_ms: Option<u64>,
    #[serde(default)]
    stepwise: bool,
    jump_ms: Option<u64>,
}

async fn move_clock(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<ClockBody>,
) -> Response {
    if let Some(jump) = body.jump_ms {
        let now = state.clock.jump(jump);
        return Json(json!({ "now": now.0 })).into_response();
    }
    if let Some(adv) = body.advance_ms {
        let now = if body.stepwise {
            let st = Arc::clone(&state);
            state
                .clock
                .advance_stepwise(adv, move || {
                    let st = Arc::clone(&st);
                    async move {
                        tokio::task::yield_now().await;
                        let _ = st.wait_idle(Duration::from_secs(10)).await;
                    }
                })
                .await
        } else {
            let now = state.clock.advance(adv);
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            let _ = state.wait_idle(Duration::from_secs(10)).await;
            now
        };
        return Json(json!({ "now": now.0 })).into_response();
    }
    Json(json!({ "now": state.clock.now().0 })).into_response()
}

#[derive(Deserialize)]
struct FocusBody {
    focused: bool,
}

async fn set_focus(State(state): State<Arc<ServerState>>, Json(body): Json<FocusBody>) -> Response {
    let Some(session) = state.bridge.session() else {
        return Json(json!({ "openThread": Value::Null })).into_response();
    };
    let open = session.focus(body.focused).and_then(|(acct_id, tid)| {
        let email = state.lookup_account_email(acct_id)?;
        let thread = tid.and_then(|t| state.lookup_gmail_thread(t));
        Some(json!({ "account": email, "thread": thread }))
    });
    Json(json!({ "openThread": open })).into_response()
}

async fn get_platform(State(state): State<Arc<ServerState>>) -> Response {
    let snap = state.recorded.lock().clone();
    Json(snap).into_response()
}

async fn clear_recorded(State(state): State<Arc<ServerState>>) -> Response {
    let mut guard = state.recorded.lock();
    let badge = guard.badge;
    *guard = super::server::RecordedState::default();
    guard.badge = badge;
    Json(Value::Null).into_response()
}

async fn list_hits(State(state): State<Arc<ServerState>>) -> Response {
    let hits = state.hits.lock().clone();
    Json(hits).into_response()
}

#[derive(Deserialize)]
struct DbContainsQuery {
    text: String,
}

async fn db_contains(
    State(state): State<Arc<ServerState>>,
    Query(q): Query<DbContainsQuery>,
) -> Response {
    let needle = q.text.as_bytes();
    let files = [
        state.data_dir.join("mail.db"),
        state.data_dir.join("mail.db-wal"),
        state.data_dir.join("mail.db-shm"),
    ];
    let found = !needle.is_empty()
        && files.iter().any(|p| {
            std::fs::read(p)
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
        });
    Json(json!({ "found": found })).into_response()
}

async fn list_draft_folders(State(state): State<Arc<ServerState>>) -> Response {
    let root = state.data_dir.join("drafts");
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            if entry.path().is_dir()
                && let Some(name) = entry.file_name().to_str()
            {
                names.push(name.to_owned());
            }
        }
    }
    names.sort();
    Json(names).into_response()
}

#[derive(Deserialize)]
struct NameBody {
    name: String,
}

async fn create_draft_folder(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<NameBody>,
) -> Response {
    let dir = state.data_dir.join("drafts").join(&body.name);
    if let Err(err) = std::fs::create_dir_all(&dir) {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, err);
    }
    if let Err(err) = std::fs::write(dir.join("orphan.bin"), b"orphan") {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, err);
    }
    Json(Value::Null).into_response()
}

async fn block_draft_folders(State(state): State<Arc<ServerState>>) -> Response {
    let root = state.data_dir.join("drafts");
    if root.is_dir() {
        crate::core::log_err(
            "remove draft folder before blocking",
            std::fs::remove_dir_all(&root),
        );
    }
    if let Err(err) = std::fs::write(&root, b"blocked") {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, err);
    }
    Json(Value::Null).into_response()
}
