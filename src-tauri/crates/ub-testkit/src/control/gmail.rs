//! Mail and Gmail-side changes: deliveries, the mailbox as Gmail sees it,
//! and changes made in other clients (labels, deletes, drafts, aliases,
//! expired tokens and history).

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ub_fake_gmail::{AddedDraft, AddedMessage, FakeMessage, FakeUser, NewAttachment, NewMessage};

use crate::server::{Ctl, Fail, Server, done};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentReq {
    filename: String,
    #[serde(default)]
    mime_type: Option<String>,
    /// Plain text content, or base64 with `base64: true`.
    #[serde(default)]
    content: String,
    #[serde(default)]
    base64: bool,
    #[serde(default)]
    content_id: Option<String>,
}

fn attachments(list: Vec<AttachmentReq>) -> Result<Vec<NewAttachment>, Fail> {
    list.into_iter()
        .map(|a| {
            let data = if a.base64 {
                base64::engine::general_purpose::STANDARD.decode(a.content)?
            } else {
                a.content.into_bytes()
            };
            Ok(NewAttachment {
                filename: a.filename,
                mime_type: a
                    .mime_type
                    .unwrap_or_else(|| "application/octet-stream".into()),
                data,
                content_id: a.content_id,
            })
        })
        .collect()
}

/// App time as the fake's dates count it.
fn app_now(s: &Server) -> u64 {
    u64::try_from(s.now().0).unwrap_or(0)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeliverReq {
    /// The mailbox the message lands in.
    account: String,
    from: String,
    #[serde(default)]
    to: Vec<String>,
    #[serde(default)]
    cc: Vec<String>,
    subject: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    html: Option<String>,
    /// The charset the text parts are labelled with; their bytes stay UTF-8.
    #[serde(default)]
    charset: Option<String>,
    /// Defaults to `INBOX`, `UNREAD`.
    #[serde(default)]
    labels: Option<Vec<String>>,
    /// Joins this Gmail thread.
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    in_reply_to: Option<String>,
    #[serde(default)]
    references: Option<String>,
    #[serde(default)]
    list_id: Option<String>,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    attachments: Vec<AttachmentReq>,
    /// Unix ms; defaults to app time.
    #[serde(default)]
    date: Option<u64>,
    /// In a batch: joins the thread of this earlier item and replies to it.
    #[serde(default)]
    after: Option<usize>,
    /// Poll at once, so the app sees the message without waiting.
    #[serde(default = "yes")]
    poll: bool,
}

fn yes() -> bool {
    true
}

fn added_json(m: &AddedMessage) -> Value {
    json!({ "id": m.id, "threadId": m.thread_id, "messageId": m.rfc822_message_id })
}

fn deliver_one(s: &Server, req: DeliverReq) -> Result<AddedMessage, Fail> {
    let user = s.user(&req.account)?;
    let date = req.date.unwrap_or_else(|| app_now(s));
    Ok(user.add_message(&NewMessage {
        from: req.from,
        to: if req.to.is_empty() {
            vec![user.email().to_owned()]
        } else {
            req.to
        },
        cc: req.cc,
        subject: req.subject,
        text: req.text,
        html: req.html,
        charset: req.charset,
        labels: req
            .labels
            .unwrap_or_else(|| vec!["INBOX".into(), "UNREAD".into()]),
        thread_id: req.thread_id,
        in_reply_to: req.in_reply_to,
        references: req.references,
        list_id: req.list_id,
        extra_headers: req.headers,
        attachments: attachments(req.attachments)?,
        internal_date: Some(date),
        ..NewMessage::default()
    }))
}

/// Puts a message in a mailbox, as if it arrived or was filed in Gmail web.
pub(crate) async fn deliver(State(s): State<Arc<Server>>, Json(req): Json<DeliverReq>) -> Ctl {
    let poll = req.poll;
    let added = deliver_one(&s, req)?;
    if poll {
        s.poll().await;
    }
    Ok(Json(added_json(&added)))
}

/// Many deliveries in one call, in order (whole seeds). An item's `after`
/// names an earlier item whose thread it joins. Polls once at the end if
/// any asks.
pub(crate) async fn deliver_batch(
    State(s): State<Arc<Server>>,
    Json(list): Json<Vec<DeliverReq>>,
) -> Ctl {
    let poll = list.iter().any(|d| d.poll);
    let mut added: Vec<AddedMessage> = Vec::with_capacity(list.len());
    for mut req in list {
        if let Some(i) = req.after {
            let earlier = added
                .get(i)
                .ok_or_else(|| Fail(format!("`after: {i}` names no earlier item")))?;
            req.thread_id = Some(earlier.thread_id.clone());
            req.in_reply_to
                .get_or_insert_with(|| earlier.rfc822_message_id.clone());
        }
        added.push(deliver_one(&s, req)?);
    }
    if poll {
        s.poll().await;
    }
    Ok(Json(json!(
        added.iter().map(added_json).collect::<Vec<_>>()
    )))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageView {
    id: String,
    thread_id: String,
    labels: Vec<String>,
    date: u64,
    snippet: String,
    headers: HashMap<String, String>,
    /// The raw RFC 822 message, for checking what was sent.
    raw: String,
}

fn message_view(m: FakeMessage) -> MessageView {
    // Of repeated headers the last one wins: the suite was written against
    // that, though control.ts says the first.
    let headers: HashMap<String, String> = m
        .headers
        .into_iter()
        .map(|(name, value)| (name.to_lowercase(), value))
        .collect();
    MessageView {
        id: m.id,
        thread_id: m.thread_id,
        labels: m.label_ids,
        date: m.internal_date,
        snippet: m.snippet,
        headers,
        raw: String::from_utf8(m.raw)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }
}

/// The mailbox as Gmail sees it: messages, drafts, labels, and sendAs.
pub(crate) async fn gmail_state(State(s): State<Arc<Server>>, Path(email): Path<String>) -> Ctl {
    let user = s.user(&email)?;
    let messages: Vec<MessageView> = user.messages().into_iter().map(message_view).collect();
    let drafts: Vec<Value> = user
        .drafts()
        .into_iter()
        .map(|d| json!({ "id": d.id, "message": message_view(d.message) }))
        .collect();
    let labels: Vec<Value> = user
        .labels()
        .into_iter()
        .map(|(id, name)| json!({ "id": id, "name": name }))
        .collect();
    let send_as: Vec<Value> = user
        .send_as()
        .into_iter()
        .map(|(email, name, primary)| {
            json!({ "email": email, "displayName": name, "primary": primary })
        })
        .collect();
    Ok(Json(json!({
        "messages": messages,
        "drafts": drafts,
        "labels": labels,
        "sendAs": send_as,
        "historyId": user.history_id(),
    })))
}

/// Gmail-side changes poll only when asked: tests usually poll as a step.
#[derive(Deserialize, Default)]
pub(crate) struct PollOpt {
    #[serde(default)]
    poll: bool,
}

#[derive(Deserialize)]
pub(crate) struct ModifyReq {
    id: String,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
    #[serde(default)]
    poll: bool,
}

/// A label change made outside the app (Gmail web, a filter).
pub(crate) async fn modify(
    State(s): State<Arc<Server>>,
    Path(email): Path<String>,
    Json(req): Json<ModifyReq>,
) -> Ctl {
    let user = s.user(&email)?;
    if user.message(&req.id).is_none() {
        return Err(Fail(format!("no message {}", req.id)));
    }
    let add: Vec<&str> = req.add.iter().map(String::as_str).collect();
    let remove: Vec<&str> = req.remove.iter().map(String::as_str).collect();
    user.modify_labels(&req.id, &add, &remove);
    if req.poll {
        s.poll().await;
    }
    Ok(done())
}

#[derive(Deserialize)]
pub(crate) struct IdReq {
    id: String,
    #[serde(default)]
    poll: bool,
}

/// A message deleted outside the app.
pub(crate) async fn delete_message(
    State(s): State<Arc<Server>>,
    Path(email): Path<String>,
    Json(req): Json<IdReq>,
) -> Ctl {
    let user = s.user(&email)?;
    if user.message(&req.id).is_none() {
        return Err(Fail(format!("no message {}", req.id)));
    }
    user.delete_message(&req.id);
    if req.poll {
        s.poll().await;
    }
    Ok(done())
}

/// A whole thread deleted outside the app.
pub(crate) async fn delete_thread(
    State(s): State<Arc<Server>>,
    Path(email): Path<String>,
    Json(req): Json<IdReq>,
) -> Ctl {
    s.user(&email)?.delete_thread(&req.id);
    if req.poll {
        s.poll().await;
    }
    Ok(done())
}

#[derive(Deserialize)]
pub(crate) struct NameReq {
    name: String,
}

/// A label created outside the app. Returns its Gmail id.
pub(crate) async fn label(
    State(s): State<Arc<Server>>,
    Path(email): Path<String>,
    Json(req): Json<NameReq>,
) -> Ctl {
    let user = s.user(&email)?;
    if user
        .labels()
        .iter()
        .any(|(_, name)| name.eq_ignore_ascii_case(&req.name))
    {
        return Err(Fail(format!("label {} exists", req.name)));
    }
    Ok(Json(json!({ "id": user.add_label(&req.name) })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SendAsReq {
    email: String,
    display_name: String,
}

/// A send-as alias added in Gmail settings.
pub(crate) async fn send_as(
    State(s): State<Arc<Server>>,
    Path(email): Path<String>,
    Json(req): Json<SendAsReq>,
) -> Ctl {
    s.user(&email)?.add_send_as(&req.email, &req.display_name);
    Ok(done())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DraftReq {
    /// Defaults to the mailbox's own address.
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Vec<String>,
    #[serde(default)]
    cc: Vec<String>,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    in_reply_to: Option<String>,
    #[serde(default)]
    references: Option<String>,
    #[serde(default)]
    headers: Vec<(String, String)>,
    #[serde(default)]
    attachments: Vec<AttachmentReq>,
    #[serde(default)]
    date: Option<u64>,
    #[serde(default)]
    poll: bool,
}

fn draft_message(s: &Server, user: &FakeUser, req: DraftReq) -> Result<NewMessage, Fail> {
    let date = req.date.unwrap_or_else(|| app_now(s));
    Ok(NewMessage {
        from: req.from.unwrap_or_else(|| user.email().to_owned()),
        to: req.to,
        cc: req.cc,
        subject: req.subject,
        text: req.text,
        html: req.html,
        thread_id: req.thread_id,
        references: req.references.or_else(|| req.in_reply_to.clone()),
        in_reply_to: req.in_reply_to,
        extra_headers: req.headers,
        attachments: attachments(req.attachments)?,
        internal_date: Some(date),
        ..NewMessage::default()
    })
}

fn draft_json(d: &AddedDraft) -> Json<Value> {
    Json(json!({
        "id": d.id,
        "messageId": d.message_id,
        "threadId": d.thread_id,
        "rfc822MessageId": d.rfc822_message_id,
    }))
}

/// A draft made in another client (Gmail web, a phone).
pub(crate) async fn create_draft(
    State(s): State<Arc<Server>>,
    Path(email): Path<String>,
    Json(req): Json<DraftReq>,
) -> Ctl {
    let user = s.user(&email)?;
    let poll = req.poll;
    let added = user.add_draft(&draft_message(&s, &user, req)?);
    if poll {
        s.poll().await;
    }
    Ok(draft_json(&added))
}

/// A draft edited in another client.
pub(crate) async fn update_draft(
    State(s): State<Arc<Server>>,
    Path((email, id)): Path<(String, String)>,
    Json(req): Json<DraftReq>,
) -> Ctl {
    let user = s.user(&email)?;
    let poll = req.poll;
    let updated = user
        .update_draft(&id, &draft_message(&s, &user, req)?)
        .ok_or_else(|| Fail(format!("no draft {id}")))?;
    if poll {
        s.poll().await;
    }
    Ok(draft_json(&updated))
}

/// A draft deleted in another client.
pub(crate) async fn delete_draft(
    State(s): State<Arc<Server>>,
    Path((email, id)): Path<(String, String)>,
    body: Option<Json<PollOpt>>,
) -> Ctl {
    if !s.user(&email)?.delete_draft(&id) {
        return Err(Fail(format!("no draft {id}")));
    }
    if body.is_some_and(|b| b.poll) {
        s.poll().await;
    }
    Ok(done())
}

/// A draft sent from another client (a phone).
pub(crate) async fn send_draft(
    State(s): State<Arc<Server>>,
    Path((email, id)): Path<(String, String)>,
    body: Option<Json<PollOpt>>,
) -> Ctl {
    let sent = s
        .user(&email)?
        .send_draft(&id)
        .ok_or_else(|| Fail(format!("no draft {id}")))?;
    if body.is_some_and(|b| b.poll) {
        s.poll().await;
    }
    Ok(Json(added_json(&sent)))
}

/// The account's access tokens expire: its next Gmail call gets a 401.
pub(crate) async fn expire_tokens(State(s): State<Arc<Server>>, Path(email): Path<String>) -> Ctl {
    s.user(&email)?.expire_access_tokens();
    Ok(done())
}

/// Gmail forgets the account's history: the next `history.list` answers 404.
pub(crate) async fn expire_history(State(s): State<Arc<Server>>, Path(email): Path<String>) -> Ctl {
    s.user(&email)?.expire_history();
    Ok(done())
}
