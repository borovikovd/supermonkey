use std::fmt;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sm_gmail::{GMAIL_BASE_URL, HttpClient, OAuthConfig, QuotaConfig, TokenFile};
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

const READ_CONNECTIONS: NonZeroUsize = match NonZeroUsize::new(4) {
    Some(n) => n,
    None => NonZeroUsize::MIN,
};

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub debug: bool,
    pub gmail: GmailConfig,
}

impl Config {
    pub const DB_FILE: &str = "supermonkey.db";
    pub const TOKEN_FILE: &str = "tokens.json";

    #[must_use]
    pub fn in_data_dir(data_dir: &Path, debug: bool, gmail: GmailConfig) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            db_path: data_dir.join(Self::DB_FILE),
            debug,
            gmail,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmailConfig {
    pub base_url: String,
    pub oauth: OAuthConfig,
    pub quota: QuotaConfig,
}

impl GmailConfig {
    #[must_use]
    pub fn google(oauth: OAuthConfig, quota: QuotaConfig) -> Self {
        Self {
            base_url: GMAIL_BASE_URL.to_owned(),
            oauth,
            quota,
        }
    }
}

pub struct Deps {
    pub platform: Arc<dyn Platform>,
    pub clock: Arc<dyn Clock>,
    pub tokens: Arc<TokenFile>,
    pub activity: Arc<Activity>,
}

impl fmt::Debug for Deps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Deps")
            .field("activity", &self.activity)
            .finish_non_exhaustive()
    }
}

pub struct App {
    actions: Actions,
    queries: Queries,
    lifecycle: Lifecycle,
    accounts: Accounts,
    mail: Mail,
    #[cfg(test)]
    writer: CoreWriter,
}

impl fmt::Debug for App {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("App").finish_non_exhaustive()
    }
}

impl App {
    pub fn start(config: Config, deps: Deps) -> Result<Self, Error> {
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
        let pool = Arc::new(ReadPool::open(&config.db_path, READ_CONNECTIONS)?);
        let received = resume_numbering(&pool, &writer)?;
        let network = Arc::new(Network::new(writer.clone(), Arc::clone(&activity)));
        let notifier = Arc::new(Notifier::new(Arc::clone(&platform), Arc::clone(&clock)));
        let snoozes = Arc::new(Kick::new(Arc::clone(&activity)));
        let workers = AccountWorkers::new(
            Arc::new(WorkerDeps {
                data_dir: config.data_dir,
                writer: writer.clone(),
                pool: Arc::clone(&pool),
                clock: Arc::clone(&clock),
                network,
                notifier: Arc::clone(&notifier),
            }),
            config.gmail,
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

    pub const fn actions(&self) -> &Actions {
        &self.actions
    }

    pub const fn queries(&self) -> &Queries {
        &self.queries
    }

    pub const fn lifecycle(&self) -> &Lifecycle {
        &self.lifecycle
    }

    pub const fn accounts(&self) -> &Accounts {
        &self.accounts
    }

    pub const fn mail(&self) -> &Mail {
        &self.mail
    }

    #[cfg(test)]
    pub(crate) const fn writer(&self) -> &CoreWriter {
        &self.writer
    }
}

fn resume_numbering(pool: &ReadPool, writer: &CoreWriter) -> Result<Arc<Received>, Error> {
    let last = pool.with(|r| r.meta().last_action())?;
    let received = Arc::new(Received::default());
    received.note(last);
    drop(writer.call(Priority::Ui, move |_, state| {
        state.sent_ack = last;
        Ok::<_, StoreError>(())
    }));
    Ok(received)
}

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
