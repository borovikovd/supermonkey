//! The core's root (ARCHITECTURE.md, section 8.1): starts the writer, the
//! read pool, and the effect router, and hands out narrow handles.

use std::fmt;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sm_gmail::{GMAIL_BASE_URL, HttpClient, OAuthConfig, QuotaConfig, TokenStore};
use sm_store::{Priority, ReadPool, StoreError};
use tokio::sync::mpsc;

use crate::accounts::Accounts;
use crate::actions::{Actions, Received};
use crate::activity::{Activity, Kick, Tasks};
use crate::clock::Clock;
use crate::effects::Router;
use crate::lifecycle::{Lifecycle, LifecycleDeps};
use crate::mail::Mail;
use crate::queries::Queries;
use crate::workers::{AccountWorkers, Network, Notifier, Searches, WorkerDeps, badge};
use crate::writer::CoreWriter;
use crate::{Error, Platform};

/// Where the core keeps its data, how it runs, and which Gmail it talks to.
#[derive(Debug, Clone)]
pub struct Config {
    /// The app's data folder: the database, and the files of attachments
    /// and drafts.
    pub data_dir: PathBuf,
    /// The SQLite file. Created, or moved aside for a new one, if needed.
    pub db_path: PathBuf,
    /// Read-only connections for window fetches, bodies, and autocomplete.
    pub read_connections: NonZeroUsize,
    /// A debug build: the UI shows its keydown-to-paint meter.
    pub debug: bool,
    /// Gmail's endpoints, the OAuth client, and the request budget.
    pub gmail: GmailConfig,
}

impl Config {
    /// The database's name in the app's data folder.
    pub const DB_FILE: &str = "supermonkey.db";
    /// The refresh-token file's name in the app's data folder, for a
    /// [`sm_gmail::TokenFile`].
    pub const TOKEN_FILE: &str = "tokens.json";

    /// The app's database in `data_dir`, with four read connections.
    #[must_use]
    pub fn in_data_dir(data_dir: &Path, debug: bool, gmail: GmailConfig) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            db_path: data_dir.join(Self::DB_FILE),
            read_connections: NonZeroUsize::new(4).unwrap_or(NonZeroUsize::MIN),
            debug,
            gmail,
        }
    }
}

/// Which Gmail the core talks to: Google's, or the fake in tests and demo
/// mode, and how fast it may.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmailConfig {
    /// The REST API's base URL.
    pub base_url: String,
    /// The OAuth client and its endpoints.
    pub oauth: OAuthConfig,
    /// The request budget per account.
    pub quota: QuotaConfig,
}

impl GmailConfig {
    /// Google's Gmail with this OAuth client and budget.
    #[must_use]
    pub fn google(oauth: OAuthConfig, quota: QuotaConfig) -> Self {
        Self {
            base_url: GMAIL_BASE_URL.to_owned(),
            oauth,
            quota,
        }
    }
}

/// The ports the core runs on (ARCHITECTURE.md, section 8.3), made by
/// whoever starts it: the Tauri shell, or the E2E server with a manual clock
/// and a recording platform.
pub struct Deps {
    /// The operating system.
    pub platform: Arc<dyn Platform>,
    /// App time.
    pub clock: Arc<dyn Clock>,
    /// Where refresh tokens live.
    pub tokens: Arc<dyn TokenStore>,
    /// Counts the core's work in progress; the same one a manual clock counts
    /// its fired timers in.
    pub activity: Arc<Activity>,
}

/// The activity only: the other ports are trait objects.
impl fmt::Debug for Deps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Deps")
            .field("activity", &self.activity)
            .finish_non_exhaustive()
    }
}

/// A running core.
pub struct App {
    actions: Actions,
    queries: Queries,
    lifecycle: Lifecycle,
    accounts: Accounts,
    mail: Mail,
    #[cfg(test)]
    writer: CoreWriter,
}

/// Its name only: it is the core's handles.
impl fmt::Debug for App {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("App").finish_non_exhaustive()
    }
}

impl App {
    /// Opens the database and starts the core: the writer thread, the read
    /// pool, and the effect router. Call it inside a tokio runtime; the
    /// core's tasks run on it. Nothing talks to Gmail until
    /// [`Lifecycle::launch`], so the E2E server can seed accounts first.
    pub fn start(config: &Config, deps: Deps) -> Result<Self, Error> {
        let runtime = tokio::runtime::Handle::try_current()?;
        let Deps {
            platform,
            clock,
            tokens,
            activity,
        } = deps;
        let tasks = Tasks::new(runtime, Arc::clone(&activity));
        let (to_router, from_writers) = mpsc::unbounded_channel();
        let writer = CoreWriter::start(
            &config.db_path,
            Arc::clone(&activity),
            to_router,
            Arc::clone(&clock),
        )?;
        let pool = Arc::new(ReadPool::open(&config.db_path, config.read_connections)?);
        let received = resume_numbering(&pool, &writer)?;
        let network = Arc::new(Network::new(writer.clone(), Arc::clone(&activity)));
        let notifier = Arc::new(Notifier::new(Arc::clone(&platform), Arc::clone(&clock)));
        let snoozes = Arc::new(Kick::new(Arc::clone(&activity)));
        let workers = AccountWorkers::new(
            Arc::new(WorkerDeps {
                data_dir: config.data_dir.clone(),
                writer: writer.clone(),
                pool: Arc::clone(&pool),
                clock: Arc::clone(&clock),
                network: Arc::clone(&network),
                notifier: Arc::clone(&notifier),
            }),
            config.gmail.clone(),
            HttpClient::new()?,
            tokens,
            tasks.clone(),
        );
        let badge = Arc::new(Kick::new(Arc::clone(&activity)));
        let router = Router {
            workers: Arc::clone(&workers),
            searches: Searches::default(),
            notifier: Arc::clone(&notifier),
            snoozes: Arc::clone(&snoozes),
            badge: Arc::clone(&badge),
        };
        tasks.spawn("effects", router.run(from_writers));
        show_badge(&tasks, badge, &pool, &platform, &clock);
        // The tests' copy, taken before the writer moves into the handles.
        #[cfg(test)]
        let tests_writer = writer.clone();
        Ok(Self {
            actions: Actions::new(
                writer.clone(),
                Arc::clone(&received),
                Arc::clone(&clock),
                tasks.clone(),
            ),
            queries: Queries::new(Arc::clone(&pool), Arc::clone(&clock)),
            lifecycle: Lifecycle::new(
                LifecycleDeps {
                    writer: writer.clone(),
                    pool: Arc::clone(&pool),
                    received,
                    clock,
                    tasks,
                    workers: Arc::clone(&workers),
                    network,
                    notifier,
                    snoozes,
                },
                config.debug,
            ),
            accounts: Accounts::new(writer, Arc::clone(&workers), Arc::clone(&platform)),
            mail: Mail::new(platform, workers, pool),
            #[cfg(test)]
            writer: tests_writer,
        })
    }

    /// UI actions.
    pub const fn actions(&self) -> &Actions {
        &self.actions
    }

    /// The UI's lookups.
    pub const fn queries(&self) -> &Queries {
        &self.queries
    }

    /// Connecting pages, launch, focus, the online flag, and quitting.
    pub const fn lifecycle(&self) -> &Lifecycle {
        &self.lifecycle
    }

    /// Signing accounts in, and waking their workers.
    pub const fn accounts(&self) -> &Accounts {
        &self.accounts
    }

    /// Links, attachments, unsubscribe, and block.
    pub const fn mail(&self) -> &Mail {
        &self.mail
    }

    /// The writer, for tests that write as sync would.
    #[cfg(test)]
    pub(crate) const fn writer(&self) -> &CoreWriter {
        &self.writer
    }
}

/// Picks action numbers up where the database left them: the last one the
/// core has received, and, as the writer's first job, the `ack` it sends.
fn resume_numbering(pool: &ReadPool, writer: &CoreWriter) -> Result<Arc<Received>, Error> {
    let last = pool.with(|r| r.meta().last_action())?;
    let received = Arc::new(Received::default());
    received.note(last);
    // The first job: the writer's `ack` starts where the database is.
    drop(writer.call(Priority::Ui, move |_, state| {
        state.sent_ack = last;
        Ok::<_, StoreError>(())
    }));
    Ok(received)
}

/// Shows the Dock badge as the database has it at launch, and starts the
/// task that keeps it so.
fn show_badge(
    tasks: &Tasks,
    badge: Arc<Kick>,
    pool: &Arc<ReadPool>,
    platform: &Arc<dyn Platform>,
    clock: &Arc<dyn Clock>,
) {
    badge.kick();
    tasks.spawn(
        "badge",
        badge::run(
            badge,
            Arc::clone(pool),
            Arc::clone(platform),
            Arc::clone(clock),
        ),
    );
}
