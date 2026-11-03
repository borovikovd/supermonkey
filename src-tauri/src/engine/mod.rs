//! The async side: Gmail sync, the op queue, the outbox, notifications.
//! It consumes writer effects and submits sync jobs back to the writer.
//! Rust is the only thing that talks to Gmail (SPEC Sync).

pub mod attachments;
pub mod auth;
pub mod contacts;
#[cfg(feature = "fake")]
pub mod demo;
pub mod drafts;
pub mod ingest;
pub mod notify;
pub mod ops;
pub mod outbox;
pub mod platform;
pub mod search;
pub mod snooze;
pub mod sync;
#[cfg(feature = "fake")]
pub mod testserver;
pub mod webbridge;
pub mod work;

use crate::LogErr as _;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

pub use platform::{NoPlatform, Platform, TauriPlatform};
use tokio::task::AbortHandle;
use ub_gmail::{AccountScheduler, Gmail, GmailError, OAuthConfig, Outcome, Scheduler, TokenStore};

use crate::clock::{Kick, now_ms};
use crate::db::ReadPool;
use crate::model::{AccountId, Millis, SubId};
use crate::writer::{EffectReceiver, Writer};

/// How long Gmail errors may go on before the account shows a quiet sync status.
const QUIET_STATUS_AFTER: Duration = Duration::from_secs(30);

/// Endpoints and credentials. Debug builds may point at the fake Gmail server.
#[derive(Clone)]
pub struct Config {
    pub gmail_base: String,
    pub oauth: OAuthConfig,
    pub quota: ub_gmail::QuotaConfig,
}

impl Config {
    pub fn from_env() -> Config {
        let mut oauth = OAuthConfig::google(env!("GOOGLE_CLIENT_ID"), env!("GOOGLE_CLIENT_SECRET"));
        let mut gmail_base = ub_gmail::GMAIL_BASE_URL.to_string();
        if cfg!(debug_assertions) {
            if let Ok(v) = std::env::var("UBERMENSCH_GMAIL_BASE") {
                gmail_base = v;
            }
            if let Ok(v) = std::env::var("UBERMENSCH_OAUTH_AUTH_URL") {
                oauth.auth_url = v;
            }
            if let Ok(v) = std::env::var("UBERMENSCH_OAUTH_TOKEN_URL") {
                oauth.token_url = v;
            }
            if let Ok(v) = std::env::var("UBERMENSCH_OAUTH_REVOKE_URL") {
                oauth.revoke_url = v;
            }
        }
        let per_min: u32 = env!("UBERMENSCH_QUOTA_PER_MIN").parse().unwrap_or(5800);
        let quota = ub_gmail::QuotaConfig {
            refill_per_minute: per_min.saturating_sub(1000).max(1000),
            ..ub_gmail::QuotaConfig::default()
        };
        Config {
            gmail_base,
            oauth,
            quota,
        }
    }
}

/// One signed-in account's Gmail side.
pub struct AccountRt {
    pub id: AccountId,
    pub email: String,
    pub gmail: Gmail,
    pub sched: AccountScheduler,
    pub ops_kick: Kick,
    pub outbox_kick: Kick,
    pub poll_kick: Kick,
    /// Wakes fetch workers waiting for the network. Kept apart from
    /// `poll_kick`, which wakes one waiter and must reach the sync loop.
    pub online_kick: Kick,
    pub stopped: AtomicBool,
    pub work: work::WorkQueue,
    /// When retrying started, for the quiet sync status after 30 s.
    pub retrying_since: AtomicI64,
    pub landed: Arc<ingest::Landed>,
    /// Counts labels created here that got their Gmail id, so a label list
    /// read before one landed does not delete it.
    pub labels_mapped: Arc<AtomicU64>,
}

impl AccountRt {
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }

    /// Ends the account's loops and workers at their next check.
    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.work.close();
    }
}

pub struct Engine {
    /// The OS side: Tauri in the app, a recorder in tests.
    pub platform: Arc<dyn Platform>,
    pub writer: Writer,
    pub reads: ReadPool,
    pub data_dir: PathBuf,
    pub http: reqwest::Client,
    pub config: Config,
    pub scheduler: Scheduler,
    pub store: Arc<dyn TokenStore>,
    pub accounts: Mutex<HashMap<AccountId, Arc<AccountRt>>>,
    pub online: AtomicBool,
    pub searches: Mutex<HashMap<SubId, AbortHandle>>,
    /// The last notification shown, for click-through.
    pub last_notification: Mutex<Option<(crate::notify::Notification, Millis)>>,
    /// When the window last lost focus (0 while focused).
    pub unfocused_since: AtomicI64,
    /// Wakes the snooze timer when a snooze time changes.
    pub snooze_kick: Kick,
    /// Cancels the sign-in waiting in the browser, if any. A new sign-in
    /// replaces one the user abandoned.
    pub sign_in_cancel: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Every task the engine started, so [`Engine::shutdown`] can stop them.
    tasks: Mutex<Vec<AbortHandle>>,
    shut_down: AtomicBool,
}

/// Builds the engine for the app window and starts it.
pub fn start(
    app: &AppHandle,
    writer: Writer,
    reads: ReadPool,
    data_dir: PathBuf,
    effects: EffectReceiver,
) {
    let config = Config::from_env();
    let store: Arc<dyn TokenStore> =
        if cfg!(debug_assertions) && std::env::var("UBERMENSCH_GMAIL_BASE").is_ok() {
            // Against the fake server, keep test tokens out of the real Keychain.
            Arc::new(ub_gmail::MemoryTokenStore::default())
        } else {
            Arc::new(ub_gmail::Keychain)
        };
    #[cfg(feature = "fake")]
    let (config, store) = demo::setup(&writer).unwrap_or((config, store));
    let engine = Engine::new(
        Arc::new(TauriPlatform(app.clone())),
        writer,
        reads,
        data_dir,
        config,
        store,
    );
    app.manage(Arc::clone(&engine));
    webbridge::start(app);
    engine.run(effects);
}

impl Engine {
    pub fn new(
        platform: Arc<dyn Platform>,
        writer: Writer,
        reads: ReadPool,
        data_dir: PathBuf,
        config: Config,
        store: Arc<dyn TokenStore>,
    ) -> Arc<Engine> {
        Arc::new(Engine {
            platform,
            writer,
            reads,
            data_dir,
            http: ub_gmail::default_http_client(),
            scheduler: Scheduler::new(config.quota.clone()),
            config,
            store,
            accounts: Mutex::new(HashMap::new()),
            online: AtomicBool::new(true),
            searches: Mutex::new(HashMap::new()),
            last_notification: Mutex::new(None),
            unfocused_since: AtomicI64::new(0),
            snooze_kick: Kick::default(),
            sign_in_cancel: Mutex::new(None),
            tasks: Mutex::new(Vec::new()),
            shut_down: AtomicBool::new(false),
        })
    }

    /// Starts accounts and the effect loop.
    pub fn run(self: &Arc<Self>, effects: EffectReceiver) {
        launch(self);
        self.spawn("dispatch", dispatch(Arc::clone(self), effects));
    }

    /// Runs `fut` as an engine task (see [`crate::clock::spawn`]). After
    /// [`Engine::shutdown`] nothing starts.
    pub fn spawn<F>(&self, label: &'static str, fut: F) -> Option<AbortHandle>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut tasks = self.tasks.lock();
        if self.shut_down.load(Ordering::Acquire) {
            return None;
        }
        let handle = crate::clock::spawn(label, fut).inner().abort_handle();
        tasks.retain(|t| !t.is_finished());
        tasks.push(handle.clone());
        Some(handle)
    }

    /// Stops every task and the writer at once, as quitting the app does. The
    /// E2E server restarts the core in-process this way.
    pub async fn shutdown(&self) {
        self.shut_down.store(true, Ordering::Release);
        for (_, rt) in self.accounts.lock().drain() {
            rt.stop();
        }
        for (_, search) in self.searches.lock().drain() {
            search.abort();
        }
        for task in self.tasks.lock().drain(..) {
            task.abort();
        }
        self.writer.stop().await;
    }

    /// Replays every account's op queue and outbox now (back online, or app time moved).
    pub fn kick_queues(&self) {
        for rt in self.accounts.lock().values() {
            rt.ops_kick.kick();
            rt.outbox_kick.kick();
        }
    }

    /// Polls every account's history now (new mail delivered in tests and demo mode).
    pub fn kick_polls(&self) {
        for rt in self.accounts.lock().values() {
            rt.poll_kick.kick();
        }
    }

    pub fn account(&self, id: AccountId) -> Option<Arc<AccountRt>> {
        self.accounts.lock().get(&id).cloned()
    }

    /// Records a Gmail outcome for the online flag and the account's sync status.
    pub fn note<T>(self: &Arc<Self>, rt: &Arc<AccountRt>, r: &Result<T, GmailError>) {
        match r {
            Ok(_) => self.note_ok(rt),
            Err(err) => self.note_err(rt, err),
        }
    }

    pub fn note_ok(&self, rt: &AccountRt) {
        rt.retrying_since.store(0, Ordering::Relaxed);
        self.set_online(true);
        self.set_status(rt.id, "idle");
    }

    pub fn note_err(self: &Arc<Self>, rt: &Arc<AccountRt>, err: &GmailError) {
        match err.outcome {
            Outcome::Offline => {
                self.set_online(false);
                self.set_status(rt.id, "offline");
            }
            Outcome::InvalidGrant => self.needs_signin(rt.id),
            Outcome::RateLimited { .. } | Outcome::ServerError { .. } | Outcome::NoResponse => {
                let now = now_ms();
                if rt
                    .retrying_since
                    .compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    // Still retrying 30 s later, with no success between: a quiet status.
                    let (e, rt) = (Arc::clone(self), Arc::clone(rt));
                    self.spawn("sync status", async move {
                        crate::clock::sleep(QUIET_STATUS_AFTER).await;
                        if rt.retrying_since.load(Ordering::Acquire) == now && !rt.is_stopped() {
                            e.set_status(rt.id, "retrying");
                        }
                    });
                }
            }
            _ => {}
        }
    }

    pub fn set_online(&self, online: bool) {
        if self.online.swap(online, Ordering::Relaxed) != online {
            self.writer.set_online(online);
            if online {
                for rt in self.accounts.lock().values() {
                    rt.ops_kick.kick();
                    rt.outbox_kick.kick();
                    rt.poll_kick.kick();
                    rt.online_kick.kick_all();
                }
            }
        }
    }

    pub fn set_status(&self, account: AccountId, status: &'static str) {
        self.writer.sync(move |ctx| {
            let n = ctx.tx.execute(
                "UPDATE accounts SET sync_status = ?2 WHERE id = ?1 AND sync_status != ?2",
                rusqlite::params![account, status],
            )?;
            if n > 0 {
                ctx.touch_app();
                ctx.touch_summary(account);
            }
            Ok(())
        });
    }

    /// `invalid_grant`: the account shows Sign in again; its queue and outbox wait.
    pub fn needs_signin(&self, account: AccountId) {
        let removed = self.accounts.lock().remove(&account);
        if let Some(rt) = removed {
            rt.stop();
        }
        self.writer.sync(move |ctx| {
            ctx.tx.execute(
                "UPDATE accounts SET needs_signin = 1 WHERE id = ?1",
                [account],
            )?;
            ctx.touch_app();
            ctx.touch_summary(account);
            Ok(())
        });
    }

    /// Starts an account's sync, queue, and outbox.
    pub fn start_account(
        self: &Arc<Self>,
        id: AccountId,
        email: String,
        tokens: Arc<ub_gmail::OAuthTokens>,
    ) {
        let gmail = Gmail::new(&self.config.gmail_base, self.http.clone(), tokens);
        let rt = Arc::new(AccountRt {
            id,
            sched: self.scheduler.account(email.as_str()),
            email,
            gmail,
            ops_kick: Kick::default(),
            outbox_kick: Kick::default(),
            poll_kick: Kick::default(),
            online_kick: Kick::default(),
            stopped: AtomicBool::new(false),
            work: work::WorkQueue::default(),
            retrying_since: AtomicI64::new(0),
            landed: Arc::default(),
            labels_mapped: Arc::default(),
        });
        let replaced = self.accounts.lock().insert(id, Arc::clone(&rt));
        if let Some(old) = replaced {
            old.stop();
        }
        self.spawn("sync", sync::run(Arc::clone(self), Arc::clone(&rt)));
        self.spawn("ops", ops::run(Arc::clone(self), Arc::clone(&rt)));
        self.spawn("outbox", outbox::run(Arc::clone(self), Arc::clone(&rt)));
        for _ in 0..work::WORKERS {
            self.spawn("worker", work::worker(Arc::clone(self), Arc::clone(&rt)));
        }
    }

    /// Stops an account's loops and wakes them so they exit now.
    pub fn stop_account(&self, id: AccountId) {
        let Some(rt) = self.accounts.lock().remove(&id) else {
            return;
        };
        rt.stop();
        rt.ops_kick.kick_all();
        rt.outbox_kick.kick_all();
        rt.poll_kick.kick_all();
        rt.online_kick.kick_all();
    }

    /// The window gained focus: poll every account, and treat focus that comes
    /// soon after a notification shown in the background as a click on it
    /// (the notification plugin does not report clicks).
    pub fn on_focus(self: &Arc<Self>) -> Option<crate::notify::Notification> {
        let accounts: Vec<Arc<AccountRt>> = self.accounts.lock().values().cloned().collect();
        for rt in accounts {
            rt.poll_kick.kick();
            // Labels and sendAs addresses refresh on focus (SPEC Sync).
            let e = Arc::clone(self);
            self.spawn("refresh", async move {
                sync::refresh_labels(&e, &rt)
                    .await
                    .log_err("refresh_labels");
                sync::refresh_send_as(&e, &rt)
                    .await
                    .log_err("refresh_send_as");
            });
        }
        let away = self.unfocused_since.swap(0, Ordering::Relaxed);
        let (n, at) = self.last_notification.lock().take()?;
        (away > 0 && at >= away && now_ms() - at < 30_000).then_some(n)
    }

    pub fn on_blur(&self) {
        self.unfocused_since.store(now_ms(), Ordering::Relaxed);
    }
}

/// On launch: start every signed-in account, wake due snoozes, settle the outbox.
fn launch(engine: &Arc<Engine>) {
    let accounts: Vec<(AccountId, String, bool)> = engine
        .reads
        .with(|c| {
            let mut stmt =
                c.prepare("SELECT id, email, needs_signin FROM accounts ORDER BY position")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .inspect_err(|err| log::warn!("read accounts at launch: {err:#}"))
        .unwrap_or_default();
    snooze::wake_due(engine);
    outbox::on_launch(engine);
    for (id, email, needs_signin) in accounts {
        if needs_signin {
            continue;
        }
        match engine.store.load_refresh_token(&email) {
            Ok(Some(token)) => {
                let tokens = Arc::new(ub_gmail::OAuthTokens::new(
                    engine.config.oauth.clone(),
                    engine.http.clone(),
                    token,
                ));
                engine.start_account(id, email, tokens);
            }
            // Fixture accounts and accounts whose token is gone stay readable offline.
            _ => log::info!("no refresh token for account {id}; not syncing"),
        }
    }
    update_badge(engine);
    engine.spawn("snooze", snooze::run(Arc::clone(engine)));
}

async fn dispatch(engine: Arc<Engine>, mut effects: EffectReceiver) {
    use crate::writer::Effect;
    while let Some(tracked) = crate::clock::parked(effects.recv()).await {
        // Busy until this effect is handled; the tasks it starts count on their own.
        let (effect, _handling) = tracked.into_parts();
        let e = &engine;
        match effect {
            Effect::Ops(a) => {
                if let Some(rt) = e.account(a) {
                    rt.ops_kick.kick();
                }
            }
            Effect::Outbox(a) => {
                if let Some(rt) = e.account(a) {
                    rt.outbox_kick.kick();
                }
            }
            Effect::FetchThread { account, thread } => {
                if let Some(rt) = e.account(account) {
                    e.spawn(
                        "fetch thread",
                        sync::fetch_opened(Arc::clone(e), rt, thread),
                    );
                }
            }
            Effect::GmailList {
                sub,
                account,
                query,
                page_token,
            } => search::start(e, sub, account, query, page_token),
            Effect::CancelSub(sub) => search::cancel(e, &sub),
            Effect::AccountRemoved { account, email } => {
                e.spawn(
                    "forget account",
                    auth::forget(Arc::clone(e), account, email),
                );
            }
            Effect::DraftParts { account, draft } => {
                if e.account(account).is_some() {
                    e.spawn("draft parts", drafts::download_parts(Arc::clone(e), draft));
                }
            }
            Effect::ReadDraft { account, draft } => {
                if let Some(rt) = e.account(account) {
                    e.spawn("read draft", drafts::read_skipped(Arc::clone(e), rt, draft));
                }
            }
            Effect::OnScreen(a) => {
                if let Some(rt) = e.account(a) {
                    e.scheduler.set_on_screen(Some(&rt.email));
                }
            }
            Effect::Badge => update_badge(e),
            Effect::Notify(n) => notify::show(e, n),
            Effect::Snoozes => e.snooze_kick.kick(),
            Effect::RefreshLabels(a) => {
                if let Some(rt) = e.account(a) {
                    let e = Arc::clone(e);
                    engine.spawn("refresh labels", async move {
                        sync::refresh_labels(&e, &rt)
                            .await
                            .log_err("refresh_labels");
                    });
                }
            }
        }
    }
}

/// The Dock badge: unread Inbox threads across accounts that are not muted.
fn update_badge(e: &Engine) {
    let count: i64 = e
        .reads
        .with(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM threads t JOIN accounts a ON a.id = t.account_id
                 WHERE t.in_inbox = 1 AND t.unread = 1 AND t.snooze_until IS NULL
                   AND NOT EXISTS (SELECT 1 FROM settings s WHERE s.account_id = a.id AND s.key = 'muted' AND s.value = 'on')",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap_or(0);
    e.platform.set_badge((count > 0).then_some(count));
}
