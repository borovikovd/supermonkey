#![expect(
    clippy::expect_used,
    reason = "A test double: a broken invariant is a test failure."
)]

mod mailbox;
mod mime;
mod routes;
mod traffic;

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use parking_lot::{Mutex, MutexGuard};
use tokio::net::{TcpListener, TcpSocket};
use tokio::sync::{Notify, oneshot, watch};
use tokio::task::JoinHandle;
use tower::ServiceExt;

pub use crate::mailbox::Filter;
use crate::mailbox::{Mailbox, Msg, SendAsEntry};
use crate::traffic::Held;
pub use crate::traffic::{Answer, LoggedRequest, RequestInfo, api_name};

pub(crate) const TOKEN_LIFETIME: Duration = Duration::from_secs(3600);

type ClockFn = dyn Fn() -> u64 + Send + Sync;

pub struct FakeGmail {
    addr: SocketAddr,
    shared: Arc<Shared>,
    offline: watch::Sender<bool>,
    listening: watch::Receiver<bool>,
    server: JoinHandle<()>,
}

impl fmt::Debug for FakeGmail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeGmail")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

pub(crate) struct Shared {
    world: Mutex<World>,
    traffic: Mutex<Traffic>,
    log: Mutex<Vec<LoggedRequest>>,
    in_flight: AtomicUsize,
    clock: Mutex<Option<Arc<ClockFn>>>,
}

#[derive(Default)]
struct Traffic {
    faults: Vec<FaultEntry>,
    next_fault: u64,
    held: Vec<Held>,
}

#[derive(Default)]
pub(crate) struct World {
    pub users: Vec<String>,
    pub mailboxes: HashMap<String, Mailbox>,
    pub refresh_tokens: HashMap<String, String>,
    pub access_tokens: HashMap<String, (String, Instant)>,
    pub auth_codes: HashMap<String, AuthCode>,
    pub invalid_grant: HashSet<String>,
    pub no_settings_scope: HashSet<String>,
    pub deny_next_auth: bool,
    pub revocations: Vec<Revocation>,
    counter: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revocation {
    pub token: String,
    pub email: String,
}

pub(crate) struct AuthCode {
    pub email: String,
    pub scope: String,
    pub redirect_uri: String,
    pub code_challenge: String,
}

impl World {
    pub(crate) const fn next(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    pub(crate) fn issue_access_token(&mut self, email: &str) -> String {
        let token = format!("ya29.fake-{}", self.next());
        let expires = Instant::now() + TOKEN_LIFETIME;
        self.access_tokens
            .insert(token.clone(), (email.to_owned(), expires));
        token
    }

    pub(crate) fn issue_refresh_token(&mut self, email: &str) -> String {
        let token = format!("1//fake-refresh-{}", self.next());
        self.refresh_tokens.insert(token.clone(), email.to_owned());
        token
    }

    pub(crate) fn token_owner(&self, token: &str) -> Option<String> {
        self.refresh_tokens
            .get(token)
            .or_else(|| self.access_tokens.get(token).map(|(email, _)| email))
            .cloned()
    }
}

impl Shared {
    pub(crate) fn world(&self) -> MutexGuard<'_, World> {
        self.world.lock()
    }

    pub(crate) fn clock_ms(&self) -> Option<u64> {
        let clock = self.clock.lock().clone();
        clock.map(|now| now())
    }

    pub(crate) fn now_ms(&self) -> u64 {
        self.clock_ms().unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        })
    }

    fn log_request(&self, info: RequestInfo, headers: &HeaderMap, body: &[u8]) -> usize {
        let at_ms = self.now_ms();
        let mut log = self.log.lock();
        let seq = log.len();
        log.push(LoggedRequest::new(seq, info, headers, body, at_ms));
        seq
    }

    fn fault_for(&self, seq: usize, request: &RequestInfo) -> Option<Plan<'_>> {
        let mut traffic = self.traffic.lock();
        let mut chosen = None;
        for entry in traffic
            .faults
            .iter_mut()
            .filter(|e| e.matcher.matches(request))
        {
            entry.seen += 1;
            if chosen.is_none() && entry.applies() {
                chosen = Some((entry.id, entry.fault.clone()));
            }
        }
        let (hold, fault) = chosen?;
        let parked = match fault {
            Fault::Hold => true,
            Fault::HoldAnswer => false,
            other @ (Fault::Drop
            | Fault::NoResponse
            | Fault::Status { .. }
            | Fault::StatusAfterApply { .. }
            | Fault::OmitThreads(_)) => return Some(Plan::Fault(other)),
        };
        let (release, released) = oneshot::channel();
        traffic.held.push(Held {
            seq,
            hold,
            parked,
            release,
        });
        if parked {
            self.set_held(seq, true);
        }
        drop(traffic);
        let ticket = Ticket {
            released,
            unhold: Unhold { shared: self, seq },
        };
        Some(if parked {
            Plan::Hold(ticket)
        } else {
            Plan::HoldAnswer(ticket)
        })
    }

    fn set_held(&self, seq: usize, held: bool) {
        if let Some(entry) = self.log.lock().get_mut(seq) {
            entry.held = held;
        }
    }

    fn park(&self, seq: usize) {
        let mut traffic = self.traffic.lock();
        if let Some(held) = traffic.held.iter_mut().find(|h| h.seq == seq && !h.parked) {
            held.parked = true;
            self.set_held(seq, true);
        }
    }

    fn answered(&self, seq: usize, answer: Answer) {
        if let Some(entry) = self.log.lock().get_mut(seq) {
            entry.answer = Some(answer);
        }
    }
}

enum Plan<'a> {
    Fault(Fault),
    Hold(Ticket<'a>),
    HoldAnswer(Ticket<'a>),
}

struct Ticket<'a> {
    released: oneshot::Receiver<Option<Fault>>,
    unhold: Unhold<'a>,
}

impl Ticket<'_> {
    async fn park(self) -> Option<Fault> {
        let Ticket { released, unhold } = self;
        unhold.shared.park(unhold.seq);
        let then = released.await.ok().flatten();
        drop(unhold);
        then
    }
}

struct Unhold<'a> {
    shared: &'a Shared,
    seq: usize,
}

impl Drop for Unhold<'_> {
    fn drop(&mut self) {
        self.shared
            .traffic
            .lock()
            .held
            .retain(|h| h.seq != self.seq);
        self.shared.set_held(self.seq, false);
    }
}

struct InFlight<'a>(&'a AtomicUsize);

impl<'a> InFlight<'a> {
    fn new(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        InFlight(count)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug, Clone)]
pub struct Matcher {
    method: Option<String>,
    path: String,
    api: Option<String>,
    account: Option<String>,
    query: Vec<(String, String)>,
    nth: Option<usize>,
    times: Option<usize>,
}

impl Matcher {
    #[must_use]
    pub fn new(method: &str, path_glob: &str) -> Self {
        Self {
            method: (method != "*").then(|| method.to_ascii_uppercase()),
            path: path_glob.to_owned(),
            api: None,
            account: None,
            query: Vec::new(),
            nth: None,
            times: None,
        }
    }

    #[must_use]
    pub fn api(name: &str) -> Self {
        Self::new("*", "*").for_api(name)
    }

    #[must_use]
    pub fn for_api(mut self, name: &str) -> Self {
        self.api = Some(name.to_owned());
        self
    }

    #[must_use]
    pub fn account(mut self, email: &str) -> Self {
        self.account = Some(email.to_lowercase());
        self
    }

    #[must_use]
    pub fn query(mut self, name: &str, value: &str) -> Self {
        self.query.push((name.to_owned(), value.to_owned()));
        self
    }

    #[must_use]
    pub const fn nth(mut self, n: usize) -> Self {
        self.nth = Some(n);
        self
    }

    #[must_use]
    pub const fn times(mut self, n: usize) -> Self {
        self.times = Some(n);
        self
    }

    #[must_use]
    pub const fn once(self) -> Self {
        self.times(1)
    }

    #[must_use]
    pub fn matches(&self, request: &RequestInfo) -> bool {
        self.method.as_deref().is_none_or(|m| m == request.method)
            && glob(&self.path, &request.path)
            && self.api.as_deref().is_none_or(|a| a == request.api)
            && self.account.as_deref().is_none_or(|a| {
                request
                    .account
                    .as_deref()
                    .is_some_and(|r| r.eq_ignore_ascii_case(a))
            })
            && (self.query.is_empty() || {
                let pairs = request.query_pairs();
                self.query.iter().all(|q| pairs.contains(q))
            })
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

#[derive(Debug, Clone)]
pub enum Fault {
    Drop,
    NoResponse,
    Status {
        status: u16,
        reason: String,
        retry_after_secs: Option<u64>,
    },
    Hold,
    HoldAnswer,
    StatusAfterApply {
        status: u16,
        reason: String,
    },
    OmitThreads(Vec<String>),
}

struct FaultEntry {
    id: u64,
    matcher: Matcher,
    fault: Fault,
    seen: usize,
}

impl FaultEntry {
    const fn applies(&self) -> bool {
        match (self.matcher.nth, self.matcher.times) {
            (Some(n), _) => self.seen == n,
            (None, Some(t)) => self.seen <= t,
            (None, None) => true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct NewMessage {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub charset: Option<String>,
    pub labels: Vec<String>,
    pub thread_id: Option<String>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub list_id: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    pub attachments: Vec<NewAttachment>,
    pub internal_date: Option<u64>,
}

impl NewMessage {
    #[must_use]
    pub fn inbox(from: &str, subject: &str, text: &str) -> Self {
        Self {
            from: from.to_owned(),
            to: vec!["me@example.com".to_owned()],
            subject: subject.to_owned(),
            text: Some(text.to_owned()),
            labels: vec!["INBOX".to_owned(), "UNREAD".to_owned()],
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct NewAttachment {
    pub filename: String,
    pub mime_type: String,
    pub data: Vec<u8>,
    pub content_id: Option<String>,
    pub inline: bool,
    pub served_inline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedMessage {
    pub id: String,
    pub thread_id: String,
    pub rfc822_message_id: String,
}

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

#[derive(Debug, Clone)]
pub struct FakeDraft {
    pub id: String,
    pub message: FakeMessage,
}

impl From<&Msg> for FakeMessage {
    fn from(msg: &Msg) -> Self {
        Self {
            id: msg.id.clone(),
            thread_id: msg.thread_id.clone(),
            label_ids: msg.label_ids.clone(),
            internal_date: msg.internal_date,
            snippet: msg.snippet.clone(),
            headers: msg.headers.clone(),
            raw: msg.raw.clone(),
        }
    }
}

impl FakeGmail {
    #[must_use]
    pub fn start() -> Self {
        let listener = bind(SocketAddr::from(([127, 0, 0, 1], 0))).expect("bind the fake");
        let addr = listener
            .local_addr()
            .expect("a bound listener has an address");
        let shared = Arc::new(Shared {
            world: Mutex::new(World::default()),
            traffic: Mutex::default(),
            log: Mutex::default(),
            in_flight: AtomicUsize::new(0),
            clock: Mutex::new(None),
        });
        let (offline, offline_rx) = watch::channel(false);
        let (listening_tx, listening) = watch::channel(true);
        let backend = Backend {
            shared: Arc::clone(&shared),
            router: routes::router(Arc::clone(&shared)),
        };
        let server = tokio::spawn(serve(listener, addr, backend, offline_rx, listening_tx));
        Self {
            addr,
            shared,
            offline,
            listening,
            server,
        }
    }

    #[must_use]
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    #[must_use]
    pub fn token_url(&self) -> String {
        format!("http://{}/token", self.addr)
    }

    #[must_use]
    pub fn revoke_url(&self) -> String {
        format!("http://{}/revoke", self.addr)
    }

    #[must_use]
    pub fn auth_url(&self) -> String {
        format!("http://{}/auth", self.addr)
    }

    #[must_use]
    pub fn add_user(&self, email: &str) -> FakeUser {
        let mut world = self.shared.world();
        world.users.push(email.to_owned());
        world
            .mailboxes
            .insert(email.to_owned(), Mailbox::new(email));
        let refresh_token = world.issue_refresh_token(email);
        FakeUser {
            shared: Arc::clone(&self.shared),
            email: email.to_owned(),
            refresh_token,
        }
    }

    pub fn expire_access_tokens(&self) {
        let now = Instant::now();
        for (_, expiry) in self.shared.world().access_tokens.values_mut() {
            *expiry = now;
        }
    }

    #[must_use]
    pub fn inject(&self, matcher: Matcher, fault: Fault) -> u64 {
        let mut traffic = self.shared.traffic.lock();
        traffic.next_fault += 1;
        let id = traffic.next_fault;
        traffic.faults.push(FaultEntry {
            id,
            matcher,
            fault,
            seen: 0,
        });
        id
    }

    pub fn clear_faults(&self) {
        self.shared.traffic.lock().faults.clear();
    }

    #[must_use]
    pub fn release(&self, id: u64, then: Option<&Fault>) -> usize {
        let mut traffic = self.shared.traffic.lock();
        traffic
            .faults
            .retain(|e| !(matches!(e.fault, Fault::Hold | Fault::HoldAnswer) && e.id == id));
        let released: Vec<Held> = traffic.held.extract_if(.., |h| h.hold == id).collect();
        drop(traffic);
        let n = released.len();
        for h in released {
            self.shared.set_held(h.seq, false);
            _ = h.release.send(then.cloned());
        }
        n
    }

    #[must_use]
    pub fn held(&self) -> usize {
        self.shared
            .traffic
            .lock()
            .held
            .iter()
            .filter(|h| h.parked)
            .count()
    }

    #[must_use]
    pub fn held_by(&self, id: u64) -> usize {
        self.shared
            .traffic
            .lock()
            .held
            .iter()
            .filter(|h| h.parked && h.hold == id)
            .count()
    }

    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.shared.in_flight.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn requests(&self) -> Vec<LoggedRequest> {
        self.shared.log.lock().clone()
    }

    #[must_use]
    pub fn count(&self, method: &str, path_glob: &str) -> usize {
        let matcher = Matcher::new(method, path_glob);
        self.shared
            .log
            .lock()
            .iter()
            .filter(|r| matcher.matches(&r.info))
            .count()
    }

    pub fn set_clock(&self, now_ms: impl Fn() -> u64 + Send + Sync + 'static) {
        *self.shared.clock.lock() = Some(Arc::new(now_ms));
    }

    pub fn deny_next_sign_in(&self) {
        self.shared.world().deny_next_auth = true;
    }

    #[must_use]
    pub fn revocations(&self) -> Vec<Revocation> {
        self.shared.world().revocations.clone()
    }

    pub async fn set_offline(&self, offline: bool) {
        self.offline.send_replace(offline);
        let mut listening = self.listening.clone();
        listening
            .wait_for(|&l| l != offline)
            .await
            .expect("fake server running");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

impl Drop for FakeGmail {
    fn drop(&mut self) {
        self.server.abort();
        self.offline.send_replace(true);
    }
}

#[derive(Clone)]
pub struct FakeUser {
    shared: Arc<Shared>,
    email: String,
    refresh_token: String,
}

impl fmt::Debug for FakeUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeUser")
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

impl FakeUser {
    fn with<T>(&self, f: impl FnOnce(&mut Mailbox) -> T) -> T {
        let mut world = self.shared.world();
        f(world.mailboxes.get_mut(&self.email).expect("user exists"))
    }

    #[must_use]
    pub fn email(&self) -> &str {
        &self.email
    }

    #[must_use]
    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }

    pub fn expire_access_tokens(&self) {
        let now = Instant::now();
        for (email, expiry) in self.shared.world().access_tokens.values_mut() {
            if *email == self.email {
                *expiry = now;
            }
        }
    }

    #[must_use]
    pub fn filters(&self) -> Vec<Filter> {
        self.with(|mb| mb.filters.clone())
    }

    pub fn set_settings_scope(&self, granted: bool) {
        let mut world = self.shared.world();
        if granted {
            world.no_settings_scope.remove(&self.email);
        } else {
            world.no_settings_scope.insert(self.email.clone());
        }
    }

    pub fn set_invalid_grant(&self, invalid: bool) {
        let mut world = self.shared.world();
        if invalid {
            world.invalid_grant.insert(self.email.clone());
        } else {
            world.invalid_grant.remove(&self.email);
        }
    }

    pub fn set_display_name(&self, name: &str) {
        self.with(|mb| name.clone_into(&mut mb.send_as[0].display_name));
    }

    #[must_use]
    pub fn send_as(&self) -> Vec<(String, String, bool)> {
        self.with(|mb| {
            mb.send_as
                .iter()
                .map(|s| (s.email.clone(), s.display_name.clone(), s.primary))
                .collect()
        })
    }

    pub fn add_send_as(&self, email: String, display_name: String) {
        self.with(|mb| {
            mb.send_as.push(SendAsEntry {
                email,
                display_name,
                primary: false,
            });
        });
    }

    #[must_use]
    pub fn add_label(&self, name: &str) -> Option<String> {
        self.with(|mb| mb.create_label(name))
    }

    #[must_use]
    pub fn labels(&self) -> Vec<(String, String)> {
        self.with(|mb| {
            mb.labels
                .iter()
                .map(|l| (l.id.clone(), l.name.clone()))
                .collect()
        })
    }

    #[must_use]
    pub fn add_message(&self, mut msg: NewMessage) -> AddedMessage {
        let now = self.shared.clock_ms();
        self.with(|mb| {
            let date = msg.internal_date.or(now).unwrap_or_else(|| mb.next_date());
            let (raw, rfc822_message_id) = built(mb, &mut msg, date);
            let labels = mb.filtered_labels(sender_email(&msg.from), msg.labels);
            let id = mb.insert_raw(raw, msg.thread_id, labels, Some(date));
            let thread_id = mb.messages[&id].thread_id.clone();
            AddedMessage {
                id,
                thread_id,
                rfc822_message_id,
            }
        })
    }

    #[must_use]
    pub fn add_draft(&self, mut msg: NewMessage) -> AddedDraft {
        let now = self.shared.clock_ms();
        self.with(|mb| {
            let date = msg.internal_date.or(now).unwrap_or_else(|| mb.next_date());
            let (raw, rfc822_message_id) = built(mb, &mut msg, date);
            let id = mb.create_draft(raw, msg.thread_id, Some(date));
            let message = mb.drafts[&id].clone();
            let thread_id = mb.messages[&message].thread_id.clone();
            AddedDraft {
                id,
                message_id: message,
                thread_id,
                rfc822_message_id,
            }
        })
    }

    #[must_use]
    pub fn update_draft(&self, draft_id: &str, mut msg: NewMessage) -> Option<AddedDraft> {
        let now = self.shared.clock_ms();
        self.with(|mb| {
            let date = msg.internal_date.or(now).unwrap_or_else(|| mb.next_date());
            let (raw, rfc822_message_id) = built(mb, &mut msg, date);
            let message = mb
                .update_draft(draft_id, raw, msg.thread_id, Some(date))
                .ok()?;
            let thread_id = mb.messages[&message].thread_id.clone();
            Some(AddedDraft {
                id: draft_id.to_owned(),
                message_id: message,
                thread_id,
                rfc822_message_id,
            })
        })
    }

    #[must_use]
    pub fn delete_draft(&self, draft_id: &str) -> bool {
        self.with(|mb| mb.delete_draft(draft_id).is_ok())
    }

    #[must_use]
    pub fn send_draft(&self, draft_id: &str) -> Option<AddedMessage> {
        let now = self.shared.clock_ms();
        self.with(|mb| {
            let id = mb.send_draft(draft_id, None, now).ok()?;
            let msg = &mb.messages[&id];
            let thread_id = msg.thread_id.clone();
            let rfc822_message_id = mime::header(&msg.headers, "Message-ID")
                .unwrap_or_default()
                .to_owned();
            Some(AddedMessage {
                id,
                thread_id,
                rfc822_message_id,
            })
        })
    }

    pub fn delete_thread(&self, thread_id: &str) {
        self.with(|mb| {
            let ids: Vec<String> = mb
                .thread_messages(thread_id)
                .iter()
                .map(|m| m.id.clone())
                .collect();
            mb.drafts.retain(|_, m| !ids.contains(m));
            for id in ids {
                mb.delete(&id).expect("listed message exists");
            }
        });
    }

    #[must_use]
    pub fn modify_labels<L: AsRef<str>>(&self, message_id: &str, add: &[L], remove: &[L]) -> bool {
        self.with(|mb| mb.modify(message_id, add, remove).is_ok())
    }

    #[must_use]
    pub fn delete_message(&self, message_id: &str) -> bool {
        self.with(|mb| {
            if !mb.messages.contains_key(message_id) {
                return false;
            }
            mb.drafts.retain(|_, m| m != message_id);
            mb.delete(message_id).is_ok()
        })
    }

    #[must_use]
    pub fn messages(&self) -> Vec<FakeMessage> {
        self.with(|mb| mb.messages.values().map(FakeMessage::from).collect())
    }

    #[must_use]
    pub fn drafts(&self) -> Vec<FakeDraft> {
        self.with(|mb| {
            mb.drafts
                .iter()
                .map(|(id, msg)| FakeDraft {
                    id: id.clone(),
                    message: FakeMessage::from(&mb.messages[msg]),
                })
                .collect()
        })
    }

    #[must_use]
    pub fn history_id(&self) -> u64 {
        self.with(|mb| mb.history_id)
    }

    pub fn expire_history(&self) {
        self.with(|mb| {
            mb.history_id += 1;
            mb.history_floor = mb.history_id;
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedDraft {
    pub id: String,
    pub message_id: String,
    pub thread_id: String,
    pub rfc822_message_id: String,
}

fn built(mb: &mut Mailbox, msg: &mut NewMessage, date: u64) -> (Vec<u8>, String) {
    let rfc822_message_id = msg
        .message_id
        .take()
        .unwrap_or_else(|| format!("<{}@fake.example>", mb.new_id()));
    (
        mime::build(msg, date, &rfc822_message_id),
        rfc822_message_id,
    )
}

fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let socket = TcpSocket::new_v4()?;
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;
    socket.listen(1024)
}

#[derive(Clone)]
struct Backend {
    shared: Arc<Shared>,
    router: Router,
}

async fn serve(
    listener: TcpListener,
    addr: SocketAddr,
    backend: Backend,
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
                        tokio::spawn(connection(stream, backend.clone(), offline_for_connections.clone()));
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

async fn connection(
    stream: tokio::net::TcpStream,
    backend: Backend,
    mut offline: watch::Receiver<bool>,
) {
    let kill = Notify::new();
    let service = hyper::service::service_fn(|req| handle(&backend, &kill, req));
    let conn =
        hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), service);
    tokio::select! {
        _ = conn => {}
        () = kill.notified() => {}
        _ = offline.wait_for(|&o| o) => {}
    }
}

async fn handle(
    backend: &Backend,
    kill: &Notify,
    req: hyper::Request<Incoming>,
) -> Result<Response, Infallible> {
    let Backend { shared, router } = backend;
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(Body::new(body), usize::MAX)
        .await
        .unwrap_or_default();
    let info = RequestInfo::new(&shared.world(), &parts, &body);
    let seq = shared.log_request(info.clone(), &parts.headers, &body);
    let _in_flight = InFlight::new(&shared.in_flight);
    let mut hold_answer = None;
    let fault = match shared.fault_for(seq, &info) {
        None => None,
        Some(Plan::Fault(fault)) => Some(fault),
        Some(Plan::Hold(ticket)) => ticket.park().await,
        Some(Plan::HoldAnswer(ticket)) => {
            hold_answer = Some(ticket);
            None
        }
    };
    let req = hyper::Request::from_parts(parts, Body::from(body));
    let route = |req: hyper::Request<Body>| router.clone().oneshot(req);
    let response = match fault {
        None | Some(Fault::Hold | Fault::HoldAnswer) => {
            route(req).await.unwrap_or_else(|e| match e {})
        }
        Some(Fault::Status {
            status,
            reason,
            retry_after_secs,
        }) => routes::google_error(status, &reason, "Injected fault", retry_after_secs),
        Some(Fault::StatusAfterApply { status, reason }) => {
            let _ = route(req).await;
            routes::google_error(status, &reason, "Injected fault", None)
        }
        Some(Fault::OmitThreads(ids)) => {
            let response = route(req).await.unwrap_or_else(|e| match e {});
            omit_threads(response, &ids).await
        }
        Some(Fault::Drop) => {
            shared.answered(seq, Answer::Dropped);
            kill.notify_one();
            return std::future::pending().await;
        }
        Some(Fault::NoResponse) => {
            let _ = route(req).await;
            shared.answered(seq, Answer::NoResponse);
            kill.notify_one();
            return std::future::pending().await;
        }
    };
    if let Some(ticket) = hold_answer {
        let _ = ticket.park().await;
    }
    shared.answered(seq, Answer::Status(response.status().as_u16()));
    Ok(response)
}

async fn omit_threads(response: Response, ids: &[String]) -> Response {
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .unwrap_or_default();
    let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    if let Some(threads) = json
        .get_mut("threads")
        .and_then(serde_json::Value::as_array_mut)
    {
        threads.retain(|t| !ids.iter().any(|id| t["id"] == id.as_str()));
    }
    (parts.status, axum::Json(json)).into_response()
}

fn sender_email(from: &str) -> &str {
    from.rsplit_once('<')
        .and_then(|(_, rest)| rest.split_once('>'))
        .map_or_else(|| from.trim(), |(addr, _)| addr.trim())
}
