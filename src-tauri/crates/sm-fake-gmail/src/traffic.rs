use axum::http::HeaderMap;
use axum::http::request::Parts;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::{Fault, World};

#[must_use]
pub fn api_name(method: &str, path: &str) -> &'static str {
    let Some(rest) = path.strip_prefix("/gmail/v1/users/") else {
        return match path {
            "/auth" => "oauth.authorize",
            "/token" => "oauth.token",
            "/revoke" => "oauth.revoke",
            _ => "unknown",
        };
    };
    let segments: Vec<&str> = rest.split('/').skip(1).collect();
    match (method, segments.as_slice()) {
        ("GET", ["profile"]) => "getProfile",
        ("GET", ["settings", "sendAs"]) => "sendAs.list",
        ("GET", ["settings", "filters"]) => "filters.list",
        ("POST", ["settings", "filters"]) => "filters.create",
        ("DELETE", ["settings", "filters", _]) => "filters.delete",
        ("GET", ["labels"]) => "labels.list",
        ("POST", ["labels"]) => "labels.create",
        ("GET", ["history"]) => "history.list",
        ("GET", ["threads"]) => "threads.list",
        ("GET", ["threads", _]) => "threads.get",
        ("POST", ["messages", "batchModify"]) => "messages.batchModify",
        ("POST", ["messages", "send"]) => "messages.send",
        ("GET", ["messages", _]) => "messages.get",
        ("POST", ["messages", _, "trash"]) => "messages.trash",
        ("GET", ["messages", _, "attachments", _]) => "attachments.get",
        ("GET", ["drafts"]) => "drafts.list",
        ("POST", ["drafts"]) => "drafts.create",
        ("POST", ["drafts", "send"]) => "drafts.send",
        ("GET", ["drafts", _]) => "drafts.get",
        ("PUT", ["drafts", _]) => "drafts.update",
        ("DELETE", ["drafts", _]) => "drafts.delete",
        _ => "unknown",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestInfo {
    pub api: &'static str,
    pub method: String,
    pub path: String,
    pub query: String,
    pub account: Option<String>,
}

impl RequestInfo {
    pub(crate) fn new(world: &World, request: &Parts, body: &[u8]) -> Self {
        let method = request.method.as_str();
        let path = request.uri.path();
        let query = request.uri.query().unwrap_or_default();
        let headers = &request.headers;
        let api = api_name(method, path);
        let form = |name: &str| {
            url::form_urlencoded::parse(body)
                .chain(url::form_urlencoded::parse(query.as_bytes()))
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        };
        let account = match api {
            "oauth.authorize" => url::form_urlencoded::parse(query.as_bytes())
                .find(|(k, _)| k == "login_hint")
                .map(|(_, v)| v.into_owned())
                .or_else(|| world.users.first().cloned()),
            "oauth.token" => form("refresh_token")
                .and_then(|t| world.refresh_tokens.get(&t).cloned())
                .or_else(|| {
                    form("code").and_then(|c| world.auth_codes.get(&c).map(|a| a.email.clone()))
                }),
            "oauth.revoke" => form("token").and_then(|t| world.token_owner(&t)),
            _ => headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .and_then(|t| world.access_tokens.get(t))
                .map(|(email, _)| email.clone()),
        };
        Self {
            api,
            method: method.to_owned(),
            path: path.to_owned(),
            query: query.to_owned(),
            account,
        }
    }

    #[must_use]
    pub fn query_pairs(&self) -> Vec<(String, String)> {
        url::form_urlencoded::parse(self.query.as_bytes())
            .into_owned()
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Status(u16),
    Dropped,
    NoResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedRequest {
    pub seq: usize,
    pub info: RequestInfo,
    pub body: Option<Value>,
    pub at_ms: u64,
    pub origin: Option<String>,
    pub user_agent: Option<String>,
    pub held: bool,
    pub answer: Option<Answer>,
}

impl LoggedRequest {
    pub(crate) fn new(
        seq: usize,
        info: RequestInfo,
        headers: &HeaderMap,
        body: &[u8],
        at_ms: u64,
    ) -> Self {
        let header = |name: axum::http::header::HeaderName| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        Self {
            seq,
            info,
            body: serde_json::from_slice(body).ok(),
            at_ms,
            origin: header(axum::http::header::ORIGIN),
            user_agent: header(axum::http::header::USER_AGENT),
            held: false,
            answer: None,
        }
    }
}

pub(crate) struct Held {
    pub seq: usize,
    pub hold: u64,
    pub parked: bool,
    pub release: oneshot::Sender<Option<Fault>>,
}
