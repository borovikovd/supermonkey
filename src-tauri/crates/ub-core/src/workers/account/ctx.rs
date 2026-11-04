//! What an account worker's tasks share (ARCHITECTURE.md, section 8.2), and
//! how each of them handles a failed Gmail call.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use ub_domain::AccountId;
use ub_gmail::{Backoff, Gmail, GmailError, Outcome};
use ub_store::{ReadPool, ReadTx, StoreError};

use super::health::Health;
use super::queue::FetchQueue;
use crate::Error;
use crate::activity::{Kick, Tasks, Tracked};
use crate::clock::Clock;
use crate::workers::network::Network;
use crate::workers::notifications::Notifier;
use crate::workers::ops::OpsLane;
use crate::writer::CoreWriter;

/// How soon a task tries again while Gmail is unreachable, unless the
/// network comes back first.
pub(crate) const OFFLINE_RETRY: Duration = Duration::from_secs(10);

/// What every account worker uses and no worker owns.
pub(crate) struct WorkerDeps {
    /// The app's data folder: attachment and draft files.
    pub(crate) data_dir: PathBuf,
    pub(crate) writer: CoreWriter,
    pub(crate) pool: Arc<ReadPool>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) network: Arc<Network>,
    /// Whether the window has focus, for which new mail notifies.
    pub(crate) notifier: Arc<Notifier>,
}

/// One running worker, as the registry tells them apart: an account that
/// signs in again gets a new worker with a new id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WorkerId(pub(crate) u64);

/// Where a worker reports that Google refused its grant.
pub(crate) type SignOutSender = mpsc::UnboundedSender<Tracked<(AccountId, WorkerId)>>;

/// The state one account's tasks share. They talk to each other only through
/// it (the kicks and the queue) and the database.
pub(crate) struct AccountCtx {
    pub(crate) account: AccountId,
    pub(crate) id: WorkerId,
    /// The account's Gmail client; every call waits in its scheduler.
    pub(crate) gmail: Gmail,
    /// Background reads.
    pub(crate) queue: FetchQueue,
    /// Wakes the poller: a poll now.
    pub(crate) poll: Kick,
    /// Wakes the ops runner: ops were queued.
    pub(crate) ops: Kick,
    /// Wakes the outbox runner: a send was queued, undone, or unblocked.
    pub(crate) outbox: Kick,
    /// What the account's op replay keeps across worker replacements.
    pub(crate) lane: Arc<OpsLane>,
    pub(crate) health: Health,
    pub(crate) deps: Arc<WorkerDeps>,
    /// The worker's task group: cancelled when it stops.
    pub(crate) tasks: Tasks,
    sign_out: SignOutSender,
}

/// What a task does after a Gmail call failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterError {
    /// Call again: the wait the outcome asks for is over.
    Retry,
    /// The item is gone or not this app's to read: drop it, with no toast.
    Drop,
    /// The account is signed out: the worker is stopping.
    Stop,
}

impl AccountCtx {
    #[expect(
        clippy::too_many_arguments,
        reason = "the worker's parts, built once by the registry"
    )]
    pub(crate) fn new(
        account: AccountId,
        id: WorkerId,
        gmail: Gmail,
        deps: Arc<WorkerDeps>,
        tasks: Tasks,
        health: Health,
        queue: FetchQueue,
        lane: Arc<OpsLane>,
        sign_out: SignOutSender,
    ) -> Self {
        Self {
            account,
            id,
            gmail,
            queue,
            poll: Kick::new(Arc::clone(tasks.activity())),
            ops: Kick::new(Arc::clone(tasks.activity())),
            outbox: Kick::new(Arc::clone(tasks.activity())),
            lane,
            health,
            deps,
            tasks,
            sign_out,
        }
    }

    /// App time.
    pub(crate) fn clock(&self) -> &dyn Clock {
        &*self.deps.clock
    }

    /// The writer.
    pub(crate) fn writer(&self) -> &CoreWriter {
        &self.deps.writer
    }

    /// Runs `read` on a read-pool connection, off the async threads.
    pub(crate) async fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce(&ReadTx<'_>) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, Error> {
        let pool = Arc::clone(&self.deps.pool);
        Ok(tokio::task::spawn_blocking(move || pool.with(read)).await??)
    }

    /// Records a call's outcome for the account's sync state.
    pub(crate) fn note<T>(&self, result: &Result<T, GmailError>) {
        match result {
            Ok(_) => self.health.succeeded(),
            Err(err) => self.health.failed(err),
        }
    }

    /// Handles a failed call: records it, waits as its outcome asks (SPEC
    /// Gmail errors), and says what to do next. A refused grant signs the
    /// account out.
    pub(crate) async fn after_error(&self, err: &GmailError, backoff: &mut Backoff) -> AfterError {
        self.health.failed(err);
        match err.outcome() {
            Outcome::InvalidGrant => {
                self.signed_out();
                AfterError::Stop
            }
            Outcome::Offline => {
                self.deps.network.wait(self.clock(), OFFLINE_RETRY).await;
                AfterError::Retry
            }
            outcome if outcome.is_transient() => {
                let wait = backoff.next_delay(err);
                tracing::debug!(account = self.account.0, %outcome, ?wait, "backing off");
                self.deps.network.wait(self.clock(), wait).await;
                AfterError::Retry
            }
            _ => AfterError::Drop,
        }
    }

    /// Google refused the grant: the account shows Sign in again, and this
    /// worker stops.
    pub(crate) fn signed_out(&self) {
        tracing::info!(
            account = self.account.0,
            "the grant was refused; signing out"
        );
        let report = self
            .tasks
            .activity()
            .track("sign out", (self.account, self.id));
        // The registry is gone only while the core shuts down.
        let _ = self.sign_out.send(report);
    }
}
