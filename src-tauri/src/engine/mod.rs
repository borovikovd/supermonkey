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
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tauri::{AppHandle, Manager};

pub use platform::{NoPlatform, Platform, TauriPlatform};
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedReceiver;
use ub_gmail::{AccountScheduler, Gmail, GmailError, OAuthConfig, Outcome, Scheduler, TokenStore};

use crate::db::ReadPool;
use crate::model::{AccountId, SubId};
use crate::writer::{Effect, Writer};

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
    pub ops_kick: Notify,
    pub outbox_kick: Notify,
    pub poll_kick: Notify,
    pub stopped: AtomicBool,
    pub work: work::WorkQueue,
    /// When retrying started, for the quiet sync status after 30 s.
    pub retrying_since: AtomicI64,
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
    pub searches: Mutex<HashMap<SubId, tauri::async_runtime::JoinHandle<()>>>,
    /// The last notification shown, for click-through.
    pub last_notification: Mutex<Option<(crate::notify::Notification, i64)>>,
    /// When the window last lost focus (0 while focused).
    pub unfocused_since: AtomicI64,
    /// Wakes the snooze timer when a snooze time changes.
    pub snooze_kick: Notify,
}

pub type EngineRef = Arc<Engine>;

/// Builds the engine for the app window and starts it.
pub fn start(
    app: &AppHandle,
    writer: Writer,
    reads: ReadPool,
    data_dir: PathBuf,
    effects: UnboundedReceiver<Effect>,
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
    app.manage(engine.clone());
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
    ) -> EngineRef {
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
            snooze_kick: Notify::new(),
        })
    }

    /// Starts accounts and the effect loop.
    pub fn run(self: &Arc<Self>, effects: UnboundedReceiver<Effect>) {
        launch(self);
        tauri::async_runtime::spawn(dispatch(self.clone(), effects));
    }

    /// Replays every account's op queue and outbox now (back online, or app time moved).
    pub fn kick_queues(&self) {
        for rt in self.accounts.lock().values() {
            rt.ops_kick.notify_one();
            rt.outbox_kick.notify_one();
        }
    }

    /// Polls every account's history now (new mail delivered in tests and demo mode).
    pub fn kick_polls(&self) {
        for rt in self.accounts.lock().values() {
            rt.poll_kick.notify_one();
        }
    }

    pub fn account(&self, id: AccountId) -> Option<Arc<AccountRt>> {
        self.accounts.lock().get(&id).cloned()
    }

    /// Records a Gmail outcome for the online flag and the account's sync status.
    pub fn note<T>(&self, rt: &AccountRt, r: &Result<T, GmailError>) {
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

    pub fn note_err(&self, rt: &AccountRt, err: &GmailError) {
        match err.outcome {
            Outcome::Offline => {
                self.set_online(false);
                self.set_status(rt.id, "offline");
            }
            Outcome::InvalidGrant => self.needs_signin(rt.id),
            Outcome::RateLimited { .. } | Outcome::ServerError { .. } | Outcome::NoResponse => {
                let now = crate::db::now_ms();
                let since = rt.retrying_since.load(Ordering::Relaxed);
                if since == 0 {
                    rt.retrying_since.store(now, Ordering::Relaxed);
                } else if now - since > 30_000 {
                    self.set_status(rt.id, "retrying");
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
                    rt.ops_kick.notify_one();
                    rt.outbox_kick.notify_one();
                    rt.poll_kick.notify_one();
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
        tokens: Arc<dyn ub_gmail::TokenSource>,
    ) {
        let gmail = Gmail::new(
            self.config.gmail_base.clone(),
            email.clone(),
            self.http.clone(),
            tokens,
        );
        let rt = Arc::new(AccountRt {
            id,
            sched: self.scheduler.account(email.clone()),
            email,
            gmail,
            ops_kick: Notify::new(),
            outbox_kick: Notify::new(),
            poll_kick: Notify::new(),
            stopped: AtomicBool::new(false),
            work: work::WorkQueue::default(),
            retrying_since: AtomicI64::new(0),
        });
        let replaced = self.accounts.lock().insert(id, rt.clone());
        if let Some(old) = replaced {
            old.stop();
        }
        tauri::async_runtime::spawn(sync::run(self.clone(), rt.clone()));
        tauri::async_runtime::spawn(ops::run(self.clone(), rt.clone()));
        tauri::async_runtime::spawn(outbox::run(self.clone(), rt.clone()));
        for _ in 0..work::WORKERS {
            tauri::async_runtime::spawn(work::worker(self.clone(), rt.clone()));
        }
    }

    /// Stops an account's loops and wakes them so they exit now.
    pub fn stop_account(&self, id: AccountId) {
        let Some(rt) = self.accounts.lock().remove(&id) else {
            return;
        };
        rt.stop();
        rt.ops_kick.notify_waiters();
        rt.outbox_kick.notify_waiters();
        rt.poll_kick.notify_waiters();
    }

    /// The window gained focus: poll every account, and treat focus that comes
    /// soon after a notification shown in the background as a click on it
    /// (the notification plugin does not report clicks).
    pub fn on_focus(self: &Arc<Self>) -> Option<crate::notify::Notification> {
        let accounts: Vec<Arc<AccountRt>> = self.accounts.lock().values().cloned().collect();
        for rt in accounts {
            rt.poll_kick.notify_one();
            // Labels and sendAs addresses refresh on focus (SPEC Sync).
            let e = self.clone();
            tauri::async_runtime::spawn(async move {
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
        (away > 0 && at >= away && crate::db::now_ms() - at < 30_000).then_some(n)
    }

    pub fn on_blur(&self) {
        self.unfocused_since
            .store(crate::db::now_ms(), Ordering::Relaxed);
    }
}

/// On launch: start every signed-in account, wake due snoozes, settle the outbox.
fn launch(engine: &EngineRef) {
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
    tauri::async_runtime::spawn(snooze::run(engine.clone()));
}

async fn dispatch(engine: EngineRef, mut effects: UnboundedReceiver<Effect>) {
    while let Some(effect) = effects.recv().await {
        let e = engine.clone();
        match effect {
            Effect::Ops(a) => {
                if let Some(rt) = e.account(a) {
                    rt.ops_kick.notify_one();
                }
            }
            Effect::Outbox(a) => {
                if let Some(rt) = e.account(a) {
                    rt.outbox_kick.notify_one();
                }
            }
            Effect::FetchThread { account, thread } => {
                if let Some(rt) = e.account(account) {
                    tauri::async_runtime::spawn(sync::fetch_opened(e, rt, thread));
                }
            }
            Effect::GmailList {
                sub,
                account,
                query,
                page_token,
            } => search::start(&e, sub, account, query, page_token),
            Effect::CancelSub(sub) => search::cancel(&e, &sub),
            Effect::AccountRemoved { account, email } => {
                tauri::async_runtime::spawn(auth::forget(e, account, email));
            }
            Effect::DraftParts { account, draft } => {
                if e.account(account).is_some() {
                    tauri::async_runtime::spawn(drafts::download_parts(e, draft));
                }
            }
            Effect::ReadDraft { account, draft } => {
                if let Some(rt) = e.account(account) {
                    tauri::async_runtime::spawn(drafts::read_skipped(e, rt, draft));
                }
            }
            Effect::OnScreen(a) => {
                if let Some(rt) = e.account(a) {
                    e.scheduler.set_on_screen(Some(&rt.email));
                }
            }
            Effect::Badge => update_badge(&e),
            Effect::Notify(n) => notify::show(&e, &n),
            Effect::Snoozes => e.snooze_kick.notify_one(),
            Effect::RefreshLabels(a) => {
                if let Some(rt) = e.account(a) {
                    tauri::async_runtime::spawn(async move {
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

/// Waits for a kick or a timeout.
pub async fn wait(notify: &Notify, timeout: Duration) {
    let _ = tokio::time::timeout(timeout, notify.notified()).await;
}
