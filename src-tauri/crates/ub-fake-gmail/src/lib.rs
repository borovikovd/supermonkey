//! A local HTTP fake of the Gmail API and Google's OAuth endpoints, for tests.
//!
//! ```no_run
//! # async fn demo() {
//! use ub_fake_gmail::*;
//! let fake = FakeGmail::start().await;
//! let me = fake.add_user("me@example.com");
//! me.add_message(NewMessage::inbox("Ann <ann@example.com>", "Hello", "Hi there"));
//! fake.inject(Matcher::new("POST", "*/drafts/send").once(), Fault::NoResponse);
//! // Point the client at fake.base_url(), fake.token_url(), fake.revoke_url(), fake.auth_url().
//! # }
//! ```
//!
//! Every Gmail route needs a bearer token the fake issued and that has not expired.
//! Each mutation appends history records and bumps the mailbox's `historyId`.
//!
//! ## Faults
//!
//! [`FakeGmail::inject`] makes matching requests misbehave: [`Fault::Status`] answers with a
//! Google-shaped error, [`Fault::Delay`] waits first, [`Fault::Drop`] closes the connection
//! without doing anything, and [`Fault::NoResponse`] does the work and then closes the
//! connection without answering. To the client both closes look like "sent, no response".
//! A real "offline" (connection refused, nothing sent) cannot depend on a request the server
//! has not read yet, so it is server-wide: [`FakeGmail::set_offline`].

#![allow(clippy::unwrap_used)] // A test double: panics are test failures.
#![allow(clippy::pedantic)] // Test support code, not shipped.

mod mailbox;
mod mime;
mod routes;

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::response::Response;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tower::ServiceExt;

use crate::mailbox::{Mailbox, SendAsEntry};

/// The fake Gmail server. Dropping it stops accepting connections.
pub struct FakeGmail {
    addr: SocketAddr,
    shared: Arc<Shared>,
    offline: watch::Sender<bool>,
    listening: watch::Receiver<bool>,
    server: JoinHandle<()>,
}

pub(crate) struct Shared {
    pub world: Mutex<World>,
    faults: Mutex<Vec<FaultEntry>>,
    log: Mutex<Vec<LoggedRequest>>,
}

pub(crate) struct World {
    /// Users in the order they were added.
    pub users: Vec<String>,
    pub mailboxes: HashMap<String, Mailbox>,
    /// Refresh token to email.
    pub refresh_tokens: HashMap<String, String>,
    /// Access token to email and expiry.
    pub access_tokens: HashMap<String, (String, Instant)>,
    pub auth_codes: HashMap<String, AuthCode>,
    pub invalid_grant: HashSet<String>,
    pub token_lifetime: Duration,
    pub last_auth_request: Option<Vec<(String, String)>>,
    counter: u64,
}

pub(crate) struct AuthCode {
    pub email: String,
    pub redirect_uri: String,
    pub code_challenge: String,
}

impl World {
    pub fn next(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    pub fn issue_access_token(&mut self, email: &str) -> String {
        let token = format!("ya29.fake-{}", self.next());
        let expires = Instant::now() + self.token_lifetime;
        self.access_tokens
            .insert(token.clone(), (email.to_owned(), expires));
        token
    }

    pub fn issue_refresh_token(&mut self, email: &str) -> String {
        let token = format!("1//fake-refresh-{}", self.next());
        self.refresh_tokens.insert(token.clone(), email.to_owned());
        token
    }
}

impl Shared {
    pub fn world(&self) -> MutexGuard<'_, World> {
        self.world.lock().unwrap()
    }
}

/// A request the fake received, faulted or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggedRequest {
    pub method: String,
    pub path: String,
    /// The raw query string, without `?`.
    pub query: String,
}

/// Picks requests by method and path.
#[derive(Debug, Clone)]
pub struct Matcher {
    method: Option<String>,
    path: String,
    nth: Option<usize>,
    times: Option<usize>,
}

impl Matcher {
    /// Requests with `method` (`"*"` for any) whose path matches `path_glob`, where `*`
    /// matches any run of characters, e.g. `"*/drafts/send"` or `"/token"`.
    pub fn new(method: &str, path_glob: &str) -> Self {
        Matcher {
            method: (method != "*").then(|| method.to_ascii_uppercase()),
            path: path_glob.to_owned(),
            nth: None,
            times: None,
        }
    }

    /// Only the `n`th matching request (1-based, counted from injection).
    pub fn nth(mut self, n: usize) -> Self {
        self.nth = Some(n);
        self
    }

    /// Only the first `n` matching requests.
    pub fn times(mut self, n: usize) -> Self {
        self.times = Some(n);
        self
    }

    /// Only the next matching request.
    pub fn once(self) -> Self {
        self.times(1)
    }

    fn matches(&self, method: &str, path: &str) -> bool {
        self.method.as_deref().is_none_or(|m| m == method) && glob(&self.path, path)
    }
}

fn glob(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((prefix, rest)) => {
            let Some(text) = text.strip_prefix(prefix) else {
                return false;
            };
            (0..=text.len())
                .filter(|&i| text.is_char_boundary(i))
                .any(|i| glob(rest, &text[i..]))
        }
    }
}

/// What a matched request suffers.
#[derive(Debug, Clone)]
pub enum Fault {
    /// Close the connection without doing anything or answering.
    Drop,
    /// Do the request's work, then close the connection without answering.
    NoResponse,
    /// Answer with this status and a Google-shaped error body, without doing anything.
    Status {
        status: u16,
        /// The `errors[0].reason`, e.g. `rateLimitExceeded`.
        reason: String,
        retry_after_secs: Option<u64>,
    },
    /// Wait, then handle the request normally.
    Delay(Duration),
}

impl Fault {
    /// A [`Fault::Status`] without `Retry-After`.
    pub fn status(status: u16, reason: &str) -> Fault {
        Fault::Status {
            status,
            reason: reason.to_owned(),
            retry_after_secs: None,
        }
    }
}

struct FaultEntry {
    matcher: Matcher,
    fault: Fault,
    seen: usize,
}

/// Simple fields for a message added with [`FakeUser::add_message`].
#[derive(Debug, Clone, Default)]
pub struct NewMessage {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
    /// Label ids, e.g. `INBOX`, `UNREAD`, or a user label's id.
    pub labels: Vec<String>,
    /// Joins this thread; otherwise the message starts a new one.
    pub thread_id: Option<String>,
    /// Defaults to a generated `<...@fake.example>`.
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub list_id: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    pub attachments: Vec<NewAttachment>,
    /// Milliseconds since the epoch. Defaults to a clock a minute past the previous message.
    pub internal_date: Option<u64>,
}

impl NewMessage {
    /// An unread inbox message to `me@example.com` with a text body.
    pub fn inbox(from: &str, subject: &str, text: &str) -> Self {
        NewMessage {
            from: from.to_owned(),
            to: vec!["me@example.com".to_owned()],
            subject: subject.to_owned(),
            text: Some(text.to_owned()),
            labels: vec!["INBOX".to_owned(), "UNREAD".to_owned()],
            ..Default::default()
        }
    }
}

/// An attachment for [`NewMessage`]. With a content id it is an inline part.
#[derive(Debug, Clone, Default)]
pub struct NewAttachment {
    pub filename: String,
    pub mime_type: String,
    pub data: Vec<u8>,
    pub content_id: Option<String>,
}

/// Ids of a message the fake stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedMessage {
    pub id: String,
    pub thread_id: String,
    /// The `Message-ID` header.
    pub rfc822_message_id: String,
}

/// A snapshot of a stored message.
#[derive(Debug, Clone)]
pub struct FakeMessage {
    pub id: String,
    pub thread_id: String,
    pub label_ids: Vec<String>,
    pub internal_date: u64,
    pub snippet: String,
    pub headers: Vec<(String, String)>,
    pub raw: Vec<u8>,
}

impl FakeMessage {
    /// The first header named `name`, case-insensitively.
    pub fn header(&self, name: &str) -> Option<&str> {
        mime::header(&self.headers, name)
    }
}

/// A snapshot of a stored draft.
#[derive(Debug, Clone)]
pub struct FakeDraft {
    pub id: String,
    pub message: FakeMessage,
}

fn snapshot(msg: &mailbox::Msg) -> FakeMessage {
    FakeMessage {
        id: msg.id.clone(),
        thread_id: msg.thread_id.clone(),
        label_ids: msg.label_ids.clone(),
        internal_date: msg.internal_date,
        snippet: msg.snippet.clone(),
        headers: msg.headers.clone(),
        raw: msg.raw.clone(),
    }
}

impl FakeGmail {
    /// Starts the fake on `127.0.0.1` on a free port.
    pub async fn start() -> FakeGmail {
        let listener = bind("127.0.0.1:0".parse().unwrap()).expect("bind the fake");
        let addr = listener.local_addr().unwrap();
        let shared = Arc::new(Shared {
            world: Mutex::new(World {
                users: Vec::new(),
                mailboxes: HashMap::new(),
                refresh_tokens: HashMap::new(),
                access_tokens: HashMap::new(),
                auth_codes: HashMap::new(),
                invalid_grant: HashSet::new(),
                token_lifetime: Duration::from_secs(3600),
                last_auth_request: None,
                counter: 0,
            }),
            faults: Mutex::new(Vec::new()),
            log: Mutex::new(Vec::new()),
        });
        let (offline, offline_rx) = watch::channel(false);
        let (listening_tx, listening) = watch::channel(true);
        let router = routes::router(shared.clone());
        let server = tokio::spawn(serve(
            listener,
            addr,
            shared.clone(),
            router,
            offline_rx,
            listening_tx,
        ));
        FakeGmail {
            addr,
            shared,
            offline,
            listening,
            server,
        }
    }

    /// The Gmail API base URL, e.g. `http://127.0.0.1:PORT`.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The OAuth token endpoint.
    pub fn token_url(&self) -> String {
        format!("http://{}/token", self.addr)
    }

    /// The OAuth revoke endpoint.
    pub fn revoke_url(&self) -> String {
        format!("http://{}/revoke", self.addr)
    }

    /// The consent page. A GET redirects straight to `redirect_uri` with a code for the user
    /// named by `login_hint`, or the first user added.
    pub fn auth_url(&self) -> String {
        format!("http://{}/auth", self.addr)
    }

    /// Adds a user with an empty mailbox (system labels only) and a refresh token.
    pub fn add_user(&self, email: &str) -> FakeUser {
        let mut world = self.shared.world();
        world.users.push(email.to_owned());
        world
            .mailboxes
            .insert(email.to_owned(), Mailbox::new(email));
        let refresh_token = world.issue_refresh_token(email);
        FakeUser {
            shared: self.shared.clone(),
            email: email.to_owned(),
            refresh_token,
        }
    }

    /// How long newly issued access tokens live. Default one hour.
    pub fn set_token_lifetime(&self, lifetime: Duration) {
        self.shared.world().token_lifetime = lifetime;
    }

    /// Expires every access token issued so far, so the next Gmail call gets a 401.
    pub fn expire_access_tokens(&self) {
        let past = Instant::now().checked_sub(Duration::from_secs(1)).unwrap();
        for (_, expiry) in self.shared.world().access_tokens.values_mut() {
            *expiry = past;
        }
    }

    /// The query parameters of the last consent-page request.
    pub fn last_auth_request(&self) -> Option<Vec<(String, String)>> {
        self.shared.world().last_auth_request.clone()
    }

    /// Adds a fault for matching requests. The first applicable fault wins.
    pub fn inject(&self, matcher: Matcher, fault: Fault) {
        self.shared.faults.lock().unwrap().push(FaultEntry {
            matcher,
            fault,
            seen: 0,
        });
    }

    /// Removes every fault.
    pub fn clear_faults(&self) {
        self.shared.faults.lock().unwrap().clear();
    }

    /// Every request received so far.
    pub fn requests(&self) -> Vec<LoggedRequest> {
        self.shared.log.lock().unwrap().clone()
    }

    /// How many requests matched `method` (`"*"` for any) and `path_glob`.
    pub fn count(&self, method: &str, path_glob: &str) -> usize {
        let matcher = Matcher::new(method, path_glob);
        self.shared
            .log
            .lock()
            .unwrap()
            .iter()
            .filter(|r| matcher.matches(&r.method, &r.path))
            .count()
    }

    /// Clears the request log.
    pub fn clear_requests(&self) {
        self.shared.log.lock().unwrap().clear();
    }

    /// Takes the server off the network or back. Offline, the port refuses connections and
    /// open connections are closed, so clients see a connect error. Returns once that holds.
    pub async fn set_offline(&self, offline: bool) {
        self.offline.send_replace(offline);
        let mut listening = self.listening.clone();
        listening
            .wait_for(|&l| l != offline)
            .await
            .expect("fake server running");
        // Let clients notice their pooled connections closed.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

impl Drop for FakeGmail {
    fn drop(&mut self) {
        self.server.abort();
        self.offline.send_replace(true);
    }
}

/// A handle on one user's mailbox and tokens.
#[derive(Clone)]
pub struct FakeUser {
    shared: Arc<Shared>,
    email: String,
    refresh_token: String,
}

impl FakeUser {
    fn with<T>(&self, f: impl FnOnce(&mut Mailbox) -> T) -> T {
        let mut world = self.shared.world();
        f(world.mailboxes.get_mut(&self.email).expect("user exists"))
    }

    /// The user's address.
    pub fn email(&self) -> &str {
        &self.email
    }

    /// The refresh token issued when the user was added.
    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }

    /// Issues a fresh access token.
    pub fn access_token(&self) -> String {
        self.shared.world().issue_access_token(&self.email)
    }

    /// While set, refreshing any of this user's tokens answers `invalid_grant`.
    pub fn set_invalid_grant(&self, invalid: bool) {
        let mut world = self.shared.world();
        if invalid {
            world.invalid_grant.insert(self.email.clone());
        } else {
            world.invalid_grant.remove(&self.email);
        }
    }

    /// Sets the primary send-as display name.
    pub fn set_display_name(&self, name: &str) {
        self.with(|mb| mb.send_as[0].display_name = name.to_owned());
    }

    /// Adds a send-as alias.
    pub fn add_send_as(&self, email: &str, display_name: &str) {
        self.with(|mb| {
            mb.send_as.push(SendAsEntry {
                email: email.to_owned(),
                display_name: display_name.to_owned(),
                primary: false,
            });
        });
    }

    /// Creates a user label and returns its id. Panics if the name exists.
    pub fn add_label(&self, name: &str) -> String {
        self.with(|mb| mb.create_label(name).expect("label name is free"))
    }

    /// Label ids and names.
    pub fn labels(&self) -> Vec<(String, String)> {
        self.with(|mb| {
            mb.labels
                .iter()
                .map(|l| (l.id.clone(), l.name.clone()))
                .collect()
        })
    }

    /// Stores a message as if it arrived, recording `messagesAdded`.
    pub fn add_message(&self, msg: NewMessage) -> AddedMessage {
        self.with(|mb| {
            let date = msg.internal_date.unwrap_or_else(|| mb.next_date());
            let rfc822_message_id = msg
                .message_id
                .clone()
                .unwrap_or_else(|| format!("<{}@fake.example>", mb.new_id()));
            let raw = mime::build(&msg, date, &rfc822_message_id);
            let id = mb.insert_raw(raw, msg.thread_id.clone(), msg.labels.clone(), Some(date));
            AddedMessage {
                thread_id: mb.messages[&id].thread_id.clone(),
                id,
                rfc822_message_id,
            }
        })
    }

    /// Stores raw RFC 822 bytes as a message.
    pub fn add_raw_message(&self, raw: &[u8], thread_id: Option<&str>, labels: &[&str]) -> String {
        self.with(|mb| {
            let labels = labels.iter().map(|l| (*l).to_owned()).collect();
            mb.insert_raw(raw.to_vec(), thread_id.map(str::to_owned), labels, None)
        })
    }

    /// Changes a message's labels as another client would, recording history.
    pub fn modify_labels(&self, message_id: &str, add: &[&str], remove: &[&str]) {
        let own = |ids: &[&str]| ids.iter().map(|l| (*l).to_owned()).collect::<Vec<_>>();
        self.with(|mb| mb.modify(message_id, &own(add), &own(remove)))
            .ok()
            .expect("message exists");
    }

    /// Deletes a message (and any draft holding it), recording `messagesDeleted`.
    pub fn delete_message(&self, message_id: &str) {
        self.with(|mb| {
            mb.drafts.retain(|_, m| m != message_id);
            mb.delete(message_id).ok().expect("message exists");
        });
    }

    /// A stored message.
    pub fn message(&self, id: &str) -> Option<FakeMessage> {
        self.with(|mb| mb.messages.get(id).map(snapshot))
    }

    /// Every stored message, in id order.
    pub fn messages(&self) -> Vec<FakeMessage> {
        self.with(|mb| mb.messages.values().map(snapshot).collect())
    }

    /// A thread's messages, oldest first.
    pub fn thread(&self, thread_id: &str) -> Vec<FakeMessage> {
        let mut messages: Vec<FakeMessage> = self
            .messages()
            .into_iter()
            .filter(|m| m.thread_id == thread_id)
            .collect();
        messages.sort_by_key(|m| m.internal_date);
        messages
    }

    /// Every draft.
    pub fn drafts(&self) -> Vec<FakeDraft> {
        self.with(|mb| {
            mb.drafts
                .iter()
                .map(|(id, msg)| FakeDraft {
                    id: id.clone(),
                    message: snapshot(&mb.messages[msg]),
                })
                .collect()
        })
    }

    /// The mailbox's current history id.
    pub fn history_id(&self) -> u64 {
        self.with(|mb| mb.history_id)
    }

    /// `history.list` with a start below `floor` answers 404.
    pub fn set_history_floor(&self, floor: u64) {
        self.with(|mb| mb.history_floor = floor);
    }
}

fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let socket = TcpSocket::new_v4()?;
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;
    socket.listen(1024)
}

/// Accepts connections until the fake goes offline, then waits and rebinds the same port.
async fn serve(
    listener: TcpListener,
    addr: SocketAddr,
    shared: Arc<Shared>,
    router: Router,
    mut offline: watch::Receiver<bool>,
    listening: watch::Sender<bool>,
) {
    let mut listener = Some(listener);
    let offline_for_connections = offline.clone();
    loop {
        if let Some(l) = &listener {
            tokio::select! {
                accepted = l.accept() => {
                    if let Ok((stream, _)) = accepted {
                        tokio::spawn(connection(stream, shared.clone(), router.clone(), offline_for_connections.clone()));
                    }
                }
                _ = offline.wait_for(|&o| o) => {
                    listener = None;
                    listening.send_replace(false);
                }
            }
        } else {
            if offline.wait_for(|&o| !o).await.is_err() {
                return;
            }
            listener = Some(bind(addr).expect("rebind the fake's port"));
            listening.send_replace(true);
        }
    }
}

/// Serves one connection until the client closes it, a fault kills it, or the fake goes offline.
async fn connection(
    stream: tokio::net::TcpStream,
    shared: Arc<Shared>,
    router: Router,
    mut offline: watch::Receiver<bool>,
) {
    let kill = Arc::new(Notify::new());
    let service_kill = kill.clone();
    let service = hyper::service::service_fn(move |req| {
        handle(shared.clone(), router.clone(), service_kill.clone(), req)
    });
    let conn =
        hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), service);
    tokio::select! {
        _ = conn => {}
        () = kill.notified() => {}
        _ = offline.wait_for(|&o| o) => {}
    }
}

async fn handle(
    shared: Arc<Shared>,
    router: Router,
    kill: Arc<Notify>,
    req: hyper::Request<Incoming>,
) -> Result<Response, Infallible> {
    let method = req.method().to_string();
    let path = req.uri().path().to_owned();
    shared.log.lock().unwrap().push(LoggedRequest {
        method: method.clone(),
        path: path.clone(),
        query: req.uri().query().unwrap_or_default().to_owned(),
    });
    let fault = {
        let mut faults = shared.faults.lock().unwrap();
        let mut chosen = None;
        for entry in faults.iter_mut() {
            if !entry.matcher.matches(&method, &path) {
                continue;
            }
            entry.seen += 1;
            let applies = match (entry.matcher.nth, entry.matcher.times) {
                (Some(n), _) => entry.seen == n,
                (None, Some(t)) => entry.seen <= t,
                (None, None) => true,
            };
            if applies && chosen.is_none() {
                chosen = Some(entry.fault.clone());
            }
        }
        chosen
    };
    let req = req.map(Body::new);
    match fault {
        None => {}
        Some(Fault::Delay(wait)) => tokio::time::sleep(wait).await,
        Some(Fault::Status {
            status,
            reason,
            retry_after_secs,
        }) => {
            return Ok(routes::google_error(
                status,
                &reason,
                "Injected fault",
                retry_after_secs,
            ));
        }
        // Never answer; the connection task drops the connection on `kill`.
        Some(Fault::Drop) => {
            kill.notify_one();
            return std::future::pending().await;
        }
        Some(Fault::NoResponse) => {
            let _ = router.oneshot(req).await;
            kill.notify_one();
            return std::future::pending().await;
        }
    }
    Ok(router.oneshot(req).await.unwrap_or_else(|e| match e {}))
}
