//! The core's root (ARCHITECTURE.md, section 8.1): starts the writer, the
//! read pool, and the effect router, and hands out narrow handles.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc;
use ub_gmail::TokenStore;
use ub_store::{Priority, ReadPool, StoreError};

use crate::actions::{Actions, Received};
use crate::activity::{Activity, Tasks};
use crate::clock::Clock;
use crate::effects::{Effect, Router};
use crate::lifecycle::Lifecycle;
use crate::queries::Queries;
use crate::writer::CoreWriter;
use crate::{Error, Platform};

/// Where the core keeps its data, and how it runs.
#[derive(Debug, Clone)]
pub struct Config {
    /// The SQLite file. Created, or moved aside for a new one, if needed.
    pub db_path: PathBuf,
    /// Read-only connections for window fetches, bodies, and autocomplete.
    pub read_connections: NonZeroUsize,
    /// A debug build: the UI shows its keydown-to-paint meter.
    pub debug: bool,
}

impl Config {
    /// The database's name in the app's data folder.
    pub const DB_FILE: &str = "ubermensch.db";

    /// The app's database in `data_dir`, with four read connections.
    pub fn in_data_dir(data_dir: &Path, debug: bool) -> Self {
        Self {
            db_path: data_dir.join(Self::DB_FILE),
            read_connections: NonZeroUsize::new(4).unwrap_or(NonZeroUsize::MIN),
            debug,
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

/// A running core.
pub struct App {
    actions: Actions,
    queries: Queries,
    lifecycle: Lifecycle,
    #[expect(
        dead_code,
        reason = "the accounts slice starts each account's worker with it"
    )]
    tokens: Arc<dyn TokenStore>,
    #[cfg(test)]
    writer: CoreWriter,
}

impl App {
    /// Opens the database and starts the core: the writer thread, the read
    /// pool, and the effect router. Call it inside a tokio runtime; the
    /// core's tasks run on it.
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
        let writer = CoreWriter::start(&config.db_path, Arc::clone(&activity), to_router.clone())?;
        let pool = Arc::new(ReadPool::open(&config.db_path, config.read_connections)?);
        let last = pool.with(|r| r.meta().last_action())?;
        let received = Arc::new(Received::default());
        received.note(last);
        // The first job: the writer's `ack` starts where the database is.
        drop(writer.call(Priority::Ui, move |_, state| {
            state.sent_ack = last;
            Ok::<_, StoreError>(())
        }));
        let router = Router {
            pool: Arc::clone(&pool),
            platform,
        };
        tasks.spawn("effects", router.run(from_writers));
        // The Dock badge as the database has it at launch. The router is
        // gone only if the runtime is shutting down.
        let _ = to_router.send(activity.track("effect", Effect::Badge));
        Ok(Self {
            actions: Actions::new(writer.clone(), Arc::clone(&received), tasks.clone()),
            queries: Queries::new(pool, clock),
            lifecycle: Lifecycle::new(writer.clone(), received, tasks, config.debug),
            tokens,
            #[cfg(test)]
            writer,
        })
    }

    /// UI actions.
    pub fn actions(&self) -> &Actions {
        &self.actions
    }

    /// The UI's lookups.
    pub fn queries(&self) -> &Queries {
        &self.queries
    }

    /// Connecting pages, the online flag, and quitting.
    pub fn lifecycle(&self) -> &Lifecycle {
        &self.lifecycle
    }

    /// The writer, for tests that write as sync would.
    #[cfg(test)]
    pub(crate) fn writer(&self) -> &CoreWriter {
        &self.writer
    }
}
