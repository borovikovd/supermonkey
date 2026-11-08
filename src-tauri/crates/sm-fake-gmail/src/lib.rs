//! A local HTTP fake of the Gmail API and Google's OAuth endpoints, for tests.
//!
//! ```no_run
//! # async fn demo() {
//! use sm_fake_gmail::*;
//! let fake = FakeGmail::start();
//! let me = fake.add_user("me@example.com");
//! me.add_message(&NewMessage::inbox("Ann <ann@example.com>", "Hello", "Hi there"));
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
//!
//! [`Fault::Hold`] parks matching requests until [`FakeGmail::release`], so a test can look at
//! the client while a request is in flight. [`Fault::HoldAnswer`] does the work first and parks
//! only the answer, so the client gets a reply that is stale by the time it arrives.
//!
//! Every request is logged ([`FakeGmail::requests`]) with its Gmail method name, account,
//! body, and answer.

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
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
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
pub use crate::traffic::{Answer, LoggedRequest, Release, RequestInfo, api_name};

/// App time for the fake: what Gmail's own clock reads (dates of new messages,
/// `newer_than:`) and when requests arrive.
type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The fake Gmail server. Dropping it stops accepting connections.
pub struct FakeGmail {
    addr: SocketAddr,
    shared: Arc<Shared>,
    offline: watch::Sender<bool>,
    listening: watch::Receiver<bool>,
    server: JoinHandle<()>,
}

pub(crate) struct Shared {
    world: Mutex<World>,
    traffic: Mutex<Traffic>,
    log: Mutex<Vec<LoggedRequest>>,
    in_flight: AtomicUsize,
    clock: Mutex<Option<Clock>>,
}

/// Faults and the requests holds have parked, under one lock: a release must never fall
/// between a request choosing a hold and joining the held list, or nothing releases it.
/// The log's `held` flag goes on only under this lock, while the request is listed, so a
/// release cannot leave it on. Lock order: `traffic`, then `log`.
#[derive(Default)]
struct Traffic {
    faults: Vec<FaultEntry>,
    /// The last fault id handed out.
    next_fault: u64,
    held: Vec<Held>,
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
    /// Users whose grant lacks `gmail.settings.basic`.
    pub no_settings_scope: HashSet<String>,
    pub token_lifetime: Duration,
    pub last_auth_request: Option<Vec<(String, String)>>,
    /// The next consent page answers `access_denied`.
    pub deny_next_auth: bool,
    pub revocations: Vec<Revocation>,
    counter: u64,
}

/// A token revoked at the revoke endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revocation {
    /// The token the client sent.
    pub token: String,
    pub email: String,
}

pub(crate) struct AuthCode {
    pub email: String,
    /// The space-separated scopes the consent page granted.
    pub scope: String,
    pub redirect_uri: String,
    pub code_challenge: String,
}

impl Default for World {
    fn default() -> Self {
        World {
            users: Vec::new(),
            mailboxes: HashMap::new(),
            refresh_tokens: HashMap::new(),
            access_tokens: HashMap::new(),
            auth_codes: HashMap::new(),
            invalid_grant: HashSet::new(),
            no_settings_scope: HashSet::new(),
            token_lifetime: Duration::from_secs(3600),
            last_auth_request: None,
            deny_next_auth: false,
            revocations: Vec::new(),
            counter: 0,
        }
    }
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

    /// Whose refresh or access token this is.
    pub fn token_owner(&self, token: &str) -> Option<String> {
        self.refresh_tokens
            .get(token)
            .or_else(|| self.access_tokens.get(token).map(|(email, _)| email))
            .cloned()
    }
}

impl Shared {
    pub fn world(&self) -> MutexGuard<'_, World> {
        self.world.lock()
    }

    /// App time, if a clock was set; otherwise mailboxes keep their own dates.
    pub fn clock_ms(&self) -> Option<u64> {
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

    /// What the first fault that applies to this request does to it. Every matching entry
    /// counts the request, applied or not, so `nth` and `times` count from injection.
    ///
    /// A hold puts the request on the held list in the same critical section, so a release
    /// either removes the hold first or finds the request there. A [`Fault::HoldAnswer`]
    /// request joins the list before its work, unparked: a release during the work lets
    /// its answer through as soon as it is ready.
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
            fault => return Some(Plan::Fault(fault)),
        };
        let (release, released) = oneshot::channel();
        traffic.held.push(Held {
            seq,
            hold,
            info: request.clone(),
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

    /// Parks a request's answer, unless a release has taken it off the held list already.
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

/// What a request suffers, as [`Shared::fault_for`] chose it.
enum Plan<'a> {
    Fault(Fault),
    /// Parked now; handled once released.
    Hold(Ticket<'a>),
    /// Handled now; the answer parks once ready.
    HoldAnswer(Ticket<'a>),
}

/// A request on the held list.
struct Ticket<'a> {
    released: oneshot::Receiver<Option<Fault>>,
    // If the client goes away first, the request leaves the held list.
    unhold: Unhold<'a>,
}

impl Ticket<'_> {
    /// Waits until released (at once if it was already), then returns what the request
    /// suffers next.
    async fn park(self) -> Option<Fault> {
        let Ticket { released, unhold } = self;
        unhold.shared.park(unhold.seq);
        let then = released.await.ok().flatten();
        drop(unhold);
        then
    }
}

/// Takes a request off the held list when its handler ends.
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

/// Counts a request in flight while its handler runs.
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

/// Picks requests by method and path, and optionally by Gmail method name, account, and
/// query parameters.
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
    /// Requests with `method` (`"*"` for any) whose path matches `path_glob`, where `*`
    /// matches any run of characters, e.g. `"*/drafts/send"` or `"/token"`.
    pub fn new(method: &str, path_glob: &str) -> Self {
        Matcher {
            method: (method != "*").then(|| method.to_ascii_uppercase()),
            path: path_glob.to_owned(),
            api: None,
            account: None,
            query: Vec::new(),
            nth: None,
            times: None,
        }
    }

    /// Requests to a Gmail method by name (see [`api_name`]), e.g. `messages.batchModify`.
    pub fn api(name: &str) -> Self {
        Matcher::new("*", "*").for_api(name)
    }

    /// Only requests to this Gmail method (see [`api_name`]).
    #[must_use]
    pub fn for_api(mut self, name: &str) -> Self {
        self.api = Some(name.to_owned());
        self
    }

    /// Only requests acting for this account.
    #[must_use]
    pub fn account(mut self, email: &str) -> Self {
        self.account = Some(email.to_lowercase());
        self
    }

    /// Only requests whose query has `name=value` (one of them, for repeated keys).
    #[must_use]
    pub fn query(mut self, name: &str, value: &str) -> Self {
        self.query.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Only the `n`th matching request (1-based, counted from injection).
    #[must_use]
    pub fn nth(mut self, n: usize) -> Self {
        self.nth = Some(n);
        self
    }

    /// Only the first `n` matching requests.
    #[must_use]
    pub fn times(mut self, n: usize) -> Self {
        self.times = Some(n);
        self
    }

    /// Only the next matching request.
    #[must_use]
    pub fn once(self) -> Self {
        self.times(1)
    }

    /// Whether this picks `request` (ignoring `nth` and `times`).
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
    /// Park the request until [`FakeGmail::release`] lets it go.
    Hold,
    /// Do the request's work now, then park the answer until [`FakeGmail::release`]:
    /// the client hears what Gmail had before anything that lands in between. A release
    /// that comes during the work lets the answer go as soon as it is ready.
    HoldAnswer,
    /// Do the request's work, then answer with this error: done, but the client hears otherwise.
    StatusAfterApply { status: u16, reason: String },
    /// Handle the request, then leave these thread ids out of a `threads.list` page.
    OmitThreads(Vec<String>),
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
    id: u64,
    matcher: Matcher,
    fault: Fault,
    /// Matching requests so far, this one included.
    seen: usize,
}

impl FaultEntry {
    fn applies(&self) -> bool {
        match (self.matcher.nth, self.matcher.times) {
            (Some(n), _) => self.seen == n,
            (None, Some(t)) => self.seen <= t,
            (None, None) => true,
        }
    }
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
    /// The charset the text parts' `Content-Type` names; default `utf-8`. Their
    /// bytes stay UTF-8, as Gmail's `format=full` can serve a `koi8-r` part.
    pub charset: Option<String>,
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
    /// `Content-Disposition: inline` with no content id, beside the body, as
    /// Apple Mail sends every file.
    pub inline: bool,
    /// Served in the message's payload (`body.data`) with no attachment id,
    /// as Gmail serves some small parts.
    pub served_inline: bool,
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

impl From<&Msg> for FakeMessage {
    fn from(msg: &Msg) -> Self {
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
}

impl FakeGmail {
    /// Starts the fake on `127.0.0.1` on a free port.
    /// Must be called inside a Tokio runtime: the server task is spawned on it.
    pub fn start() -> FakeGmail {
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
        let router = routes::router(Arc::clone(&shared));
        let server = tokio::spawn(serve(
            listener,
            addr,
            Arc::clone(&shared),
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
            shared: Arc::clone(&self.shared),
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
        // A token lives while its expiry is after now, so now is already too late.
        let now = Instant::now();
        for (_, expiry) in self.shared.world().access_tokens.values_mut() {
            *expiry = now;
        }
    }

    /// The query parameters of the last consent-page request.
    pub fn last_auth_request(&self) -> Option<Vec<(String, String)>> {
        self.shared.world().last_auth_request.clone()
    }

    /// Adds a fault for matching requests. The first applicable fault wins. Returns its id,
    /// which [`Release::Hold`] names.
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

    /// Removes every fault, holds included. Requests already held stay held.
    pub fn clear_faults(&self) {
        self.shared.traffic.lock().faults.clear();
    }

    /// Ends holds and lets the requests they parked go on: handled normally, or suffering
    /// `then`. Returns how many requests it released, answers of [`Fault::HoldAnswer`]
    /// still being worked on included.
    pub fn release(&self, which: &Release, then: Option<&Fault>) -> usize {
        let mut traffic = self.shared.traffic.lock();
        traffic.faults.retain(|e| {
            !(matches!(e.fault, Fault::Hold | Fault::HoldAnswer)
                && which.covers(e.id, e.matcher.api.as_deref()))
        });
        let released: Vec<Held> = traffic
            .held
            .extract_if(.., |h| which.covers(h.hold, Some(h.info.api)))
            .collect();
        drop(traffic);
        let n = released.len();
        for h in released {
            self.shared.set_held(h.seq, false);
            // The handler may be gone already (its connection closed).
            let _ = h.release.send(then.cloned());
        }
        n
    }

    /// Requests parked by a hold now. A [`Fault::HoldAnswer`] request counts once its work
    /// is done and its answer waits.
    pub fn held(&self) -> usize {
        self.shared
            .traffic
            .lock()
            .held
            .iter()
            .filter(|h| h.parked)
            .count()
    }

    /// Requests parked now by the holds `which` names.
    pub fn held_by(&self, which: &Release) -> usize {
        self.shared
            .traffic
            .lock()
            .held
            .iter()
            .filter(|h| h.parked && which.covers(h.hold, Some(h.info.api)))
            .count()
    }

    /// Requests being handled now, held ones included.
    pub fn in_flight(&self) -> usize {
        self.shared.in_flight.load(Ordering::Acquire)
    }

    /// Every request received so far.
    pub fn requests(&self) -> Vec<LoggedRequest> {
        self.shared.log.lock().clone()
    }

    /// How many requests matched `method` (`"*"` for any) and `path_glob`.
    pub fn count(&self, method: &str, path_glob: &str) -> usize {
        let matcher = Matcher::new(method, path_glob);
        self.shared
            .log
            .lock()
            .iter()
            .filter(|r| matcher.matches(&r.info))
            .count()
    }

    /// Sets the fake's clock (Unix ms): the date of messages Gmail creates (sends, drafts),
    /// what `newer_than:` counts from, and when requests are logged. Default: the system
    /// clock, with new messages a minute apart.
    pub fn set_clock(&self, now_ms: impl Fn() -> u64 + Send + Sync + 'static) {
        *self.shared.clock.lock() = Some(Arc::new(now_ms));
    }

    /// The next consent page answers Google's `access_denied` instead of a code.
    pub fn deny_next_sign_in(&self) {
        self.shared.world().deny_next_auth = true;
    }

    /// Tokens revoked at Google so far, oldest first.
    pub fn revocations(&self) -> Vec<Revocation> {
        self.shared.world().revocations.clone()
    }

    /// Clears the request log.
    pub fn clear_requests(&self) {
        self.shared.log.lock().clear();
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

    /// Expires every access token of this user, so its next Gmail call gets a 401.
    pub fn expire_access_tokens(&self) {
        // A token lives while its expiry is after now, so now is already too late.
        let now = Instant::now();
        for (email, expiry) in self.shared.world().access_tokens.values_mut() {
            if *email == self.email {
                *expiry = now;
            }
        }
    }

    /// While set, refreshing any of this user's tokens answers `invalid_grant`.
    /// The user's Gmail filters, oldest first.
    pub fn filters(&self) -> Vec<Filter> {
        self.with(|mb| mb.filters.clone())
    }

    /// Whether the user's grant includes `gmail.settings.basic`. Off, as for an
    /// account signed in before the app asked for it, filter calls answer 403
    /// until a sign-in grants it.
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

    /// Sets the primary send-as display name.
    pub fn set_display_name(&self, name: &str) {
        self.with(|mb| name.clone_into(&mut mb.send_as[0].display_name));
    }

    /// Every send-as address: email, display name, and whether it is the primary one.
    pub fn send_as(&self) -> Vec<(String, String, bool)> {
        self.with(|mb| {
            mb.send_as
                .iter()
                .map(|s| (s.email.clone(), s.display_name.clone(), s.primary))
                .collect()
        })
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
    pub fn add_message(&self, msg: &NewMessage) -> AddedMessage {
        let now = self.shared.clock_ms();
        self.with(|mb| {
            let date = msg.internal_date.or(now).unwrap_or_else(|| mb.next_date());
            let (raw, rfc822_message_id) = built(mb, msg, date);
            let labels = mb.filtered_labels(sender_email(&msg.from), msg.labels.clone());
            let id = mb.insert_raw(raw, msg.thread_id.clone(), labels, Some(date));
            AddedMessage {
                thread_id: mb.messages[&id].thread_id.clone(),
                id,
                rfc822_message_id,
            }
        })
    }

    /// A draft made by another client (Gmail on the web, a phone). Labels are ignored.
    pub fn add_draft(&self, msg: &NewMessage) -> AddedDraft {
        let now = self.shared.clock_ms();
        self.with(|mb| {
            let date = msg.internal_date.or(now).unwrap_or_else(|| mb.next_date());
            let (raw, rfc822_message_id) = built(mb, msg, date);
            let id = mb.create_draft(raw, msg.thread_id.clone(), Some(date));
            let message = mb.drafts[&id].clone();
            AddedDraft {
                thread_id: mb.messages[&message].thread_id.clone(),
                id,
                message_id: message,
                rfc822_message_id,
            }
        })
    }

    /// Replaces a draft's message, as another client editing it. `None` if it is gone.
    pub fn update_draft(&self, draft_id: &str, msg: &NewMessage) -> Option<AddedDraft> {
        let now = self.shared.clock_ms();
        self.with(|mb| {
            let date = msg.internal_date.or(now).unwrap_or_else(|| mb.next_date());
            let (raw, rfc822_message_id) = built(mb, msg, date);
            let message = mb
                .update_draft(draft_id, raw, msg.thread_id.clone(), Some(date))
                .ok()?;
            Some(AddedDraft {
                thread_id: mb.messages[&message].thread_id.clone(),
                id: draft_id.to_owned(),
                message_id: message,
                rfc822_message_id,
            })
        })
    }

    /// Deletes a draft, as another client. Returns false if it was gone.
    pub fn delete_draft(&self, draft_id: &str) -> bool {
        self.with(|mb| mb.delete_draft(draft_id).is_ok())
    }

    /// Sends a draft from another client (a phone). Returns the sent message, or `None` if
    /// the draft was gone.
    pub fn send_draft(&self, draft_id: &str) -> Option<AddedMessage> {
        let now = self.shared.clock_ms();
        self.with(|mb| {
            let id = mb.send_draft(draft_id, None, now).ok()?;
            let msg = &mb.messages[&id];
            Some(AddedMessage {
                thread_id: msg.thread_id.clone(),
                rfc822_message_id: mime::header(&msg.headers, "Message-ID")
                    .unwrap_or_default()
                    .to_owned(),
                id,
            })
        })
    }

    /// Deletes every message of a thread (and drafts in it), recording `messagesDeleted`.
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

    /// Stores raw RFC 822 bytes as a message.
    pub fn add_raw_message(&self, raw: &[u8], thread_id: Option<&str>, labels: &[&str]) -> String {
        self.with(|mb| {
            let thread_id = thread_id.map(str::to_owned);
            mb.insert_raw(raw.to_vec(), thread_id, owned(labels), None)
        })
    }

    /// Changes a message's labels as another client would, recording history.
    pub fn modify_labels(&self, message_id: &str, add: &[&str], remove: &[&str]) {
        self.with(|mb| mb.modify(message_id, add, remove))
            .expect("message exists");
    }

    /// Deletes a message (and any draft holding it), recording `messagesDeleted`.
    pub fn delete_message(&self, message_id: &str) {
        self.with(|mb| {
            mb.drafts.retain(|_, m| m != message_id);
            mb.delete(message_id).expect("message exists");
        });
    }

    /// A stored message.
    pub fn message(&self, id: &str) -> Option<FakeMessage> {
        self.with(|mb| mb.messages.get(id).map(FakeMessage::from))
    }

    /// Every stored message, in id order.
    pub fn messages(&self) -> Vec<FakeMessage> {
        self.with(|mb| mb.messages.values().map(FakeMessage::from).collect())
    }

    /// A thread's messages, oldest first.
    pub fn thread(&self, thread_id: &str) -> Vec<FakeMessage> {
        self.with(|mb| {
            mb.thread_messages(thread_id)
                .into_iter()
                .map(FakeMessage::from)
                .collect()
        })
    }

    /// Every draft.
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

    /// The mailbox's current history id.
    pub fn history_id(&self) -> u64 {
        self.with(|mb| mb.history_id)
    }

    /// `history.list` with a start below `floor` answers 404.
    pub fn set_history_floor(&self, floor: u64) {
        self.with(|mb| mb.history_floor = floor);
    }

    /// Expires all history so far, as Gmail does after about a week: any `history.list`
    /// from an older cursor answers 404, and `getProfile` gives a cursor that works.
    pub fn expire_history(&self) {
        self.with(|mb| {
            mb.history_id += 1;
            mb.history_floor = mb.history_id;
        });
    }
}

/// Ids of a draft the fake stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedDraft {
    pub id: String,
    pub message_id: String,
    pub thread_id: String,
    /// The `Message-ID` header.
    pub rfc822_message_id: String,
}

/// RFC 822 bytes for `msg` and its `Message-ID`.
fn built(mb: &mut Mailbox, msg: &NewMessage, date: u64) -> (Vec<u8>, String) {
    let rfc822_message_id = msg
        .message_id
        .clone()
        .unwrap_or_else(|| format!("<{}@fake.example>", mb.new_id()));
    (
        mime::build(msg, date, &rfc822_message_id),
        rfc822_message_id,
    )
}

fn owned(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|&id| id.to_owned()).collect()
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
                        tokio::spawn(connection(stream, Arc::clone(&shared), router.clone(), offline_for_connections.clone()));
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
    let service_kill = Arc::clone(&kill);
    let service = hyper::service::service_fn(move |req| {
        handle(
            Arc::clone(&shared),
            router.clone(),
            Arc::clone(&service_kill),
            req,
        )
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
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(Body::new(body), usize::MAX)
        .await
        .unwrap_or_default();
    let query = parts.uri.query().unwrap_or_default();
    let info = RequestInfo::new(
        &shared.world(),
        parts.method.as_str(),
        parts.uri.path(),
        query,
        &parts.headers,
        &body,
    );
    // The clock is the caller's code: never run it under the log lock.
    let at_ms = shared.now_ms();
    let seq = {
        let mut log = shared.log.lock();
        let seq = log.len();
        log.push(LoggedRequest::new(
            seq,
            info.clone(),
            &parts.headers,
            &body,
            at_ms,
        ));
        seq
    };
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
    let response = match fault {
        None | Some(Fault::Hold | Fault::HoldAnswer) => {
            router.oneshot(req).await.unwrap_or_else(|e| match e {})
        }
        Some(Fault::Delay(wait)) => {
            tokio::time::sleep(wait).await;
            router.oneshot(req).await.unwrap_or_else(|e| match e {})
        }
        Some(Fault::Status {
            status,
            reason,
            retry_after_secs,
        }) => routes::google_error(status, &reason, "Injected fault", retry_after_secs),
        Some(Fault::StatusAfterApply { status, reason }) => {
            let _ = router.oneshot(req).await;
            routes::google_error(status, &reason, "Injected fault", None)
        }
        Some(Fault::OmitThreads(ids)) => {
            let response = router.oneshot(req).await.unwrap_or_else(|e| match e {});
            omit_threads(response, &ids).await
        }
        // Never answer; the connection task drops the connection on `kill`.
        Some(Fault::Drop) => {
            shared.answered(seq, Answer::Dropped);
            kill.notify_one();
            return std::future::pending().await;
        }
        Some(Fault::NoResponse) => {
            let _ = router.oneshot(req).await;
            shared.answered(seq, Answer::NoResponse);
            kill.notify_one();
            return std::future::pending().await;
        }
    };
    if let Some(ticket) = hold_answer {
        // The release's outcome does not apply: the work is already done.
        let _ = ticket.park().await;
    }
    shared.answered(seq, Answer::Status(response.status().as_u16()));
    Ok(response)
}

/// A `threads.list` answer without the threads `ids`.
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

/// The address in a `From` value: inside angle brackets, or the whole value.
fn sender_email(from: &str) -> &str {
    from.rsplit_once('<')
        .and_then(|(_, rest)| rest.split_once('>'))
        .map_or(from.trim(), |(addr, _)| addr.trim())
}
