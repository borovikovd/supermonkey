//! What each request is: its Gmail method, the account it acts for, and its
//! entry in the request log. Also the requests a hold has parked.

use axum::http::HeaderMap;
use serde_json::Value;
use tokio::sync::oneshot;

use crate::{Fault, World};

/// The Gmail API method a request calls, e.g. `messages.batchModify`, or the
/// OAuth endpoint it reaches (`oauth.authorize`, `oauth.token`, `oauth.revoke`).
pub fn api_name(method: &str, path: &str) -> &'static str {
    let Some(rest) = path.strip_prefix("/gmail/v1/users/") else {
        return match path {
            "/auth" => "oauth.authorize",
            "/token" => "oauth.token",
            "/revoke" => "oauth.revoke",
            _ => "unknown",
        };
    };
    // The first segment is the user (`me` or an address).
    let segments: Vec<&str> = rest.split('/').skip(1).collect();
    match (method, segments.as_slice()) {
        ("GET", ["profile"]) => "getProfile",
        ("GET", ["settings", "sendAs"]) => "sendAs.list",
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

/// A request as faults and holds see it, before it is handled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestInfo {
    /// See [`api_name`].
    pub api: &'static str,
    pub method: String,
    pub path: String,
    /// The raw query string, without `?`.
    pub query: String,
    /// The account it acts for: the bearer token's user, or the user an OAuth
    /// request names. `None` when the fake cannot tell.
    pub account: Option<String>,
}

impl RequestInfo {
    pub(crate) fn new(
        world: &World,
        method: &str,
        path: &str,
        query: &str,
        headers: &HeaderMap,
        body: &[u8],
    ) -> RequestInfo {
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
        RequestInfo {
            api,
            method: method.to_owned(),
            path: path.to_owned(),
            query: query.to_owned(),
            account,
        }
    }

    /// The query parameters, keeping repeated keys.
    pub fn query_pairs(&self) -> Vec<(String, String)> {
        url::form_urlencoded::parse(self.query.as_bytes())
            .into_owned()
            .collect()
    }
}

/// How the fake answered a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// An HTTP status: 2xx for success, else an error body.
    Status(u16),
    /// The connection closed without an answer and without doing anything.
    Dropped,
    /// The work was done, then the connection closed without an answer.
    NoResponse,
}

/// A request the fake received, in arrival order.
#[derive(Debug, Clone, PartialEq)]
pub struct LoggedRequest {
    /// 0-based position in the log.
    pub seq: usize,
    /// The request as a matcher sees it.
    pub info: RequestInfo,
    /// The body, when it is JSON.
    pub body: Option<Value>,
    /// App time when it arrived (see [`crate::FakeGmail::set_clock`]), Unix ms.
    pub at_ms: u64,
    /// The `Origin` and `User-Agent` headers, if sent.
    pub origin: Option<String>,
    pub user_agent: Option<String>,
    /// Parked by a hold right now.
    pub held: bool,
    /// `None` while it is still being handled.
    pub answer: Option<Answer>,
}

impl LoggedRequest {
    pub(crate) fn new(
        seq: usize,
        info: RequestInfo,
        headers: &HeaderMap,
        body: &[u8],
        at_ms: u64,
    ) -> LoggedRequest {
        let header = |name: axum::http::header::HeaderName| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        LoggedRequest {
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

/// A request a hold has parked, waiting for [`crate::FakeGmail::release`].
pub(crate) struct Held {
    pub seq: usize,
    /// The hold fault that parked it.
    pub hold: u64,
    pub info: RequestInfo,
    /// Waiting now. A [`Fault::HoldAnswer`] request is listed from when it chose the hold,
    /// so a release during its work is not lost, but parks only once its answer is ready.
    pub parked: bool,
    /// Receives what happens next: a fault to suffer, or `None` to be handled normally.
    pub release: oneshot::Sender<Option<Fault>>,
}

/// Which held requests to let go.
#[derive(Debug, Clone)]
pub enum Release {
    /// Every hold, and every request held.
    All,
    /// One hold (by the id [`crate::FakeGmail::inject`] returned) and what it parked.
    Hold(u64),
    /// Holds on this Gmail method, and the requests to it they parked.
    Api(String),
}

impl Release {
    /// Whether this names hold `id` (or a hold on Gmail method `api`).
    pub(crate) fn covers(&self, id: u64, api: Option<&str>) -> bool {
        match self {
            Release::All => true,
            Release::Hold(h) => *h == id,
            Release::Api(name) => api == Some(name.as_str()),
        }
    }
}
