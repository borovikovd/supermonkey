//! What an account worker's tasks share (ARCHITECTURE.md, section 8.2), and
//! how a background read is retried.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sm_domain::AccountId;
use sm_gmail::{Backoff, Gmail, GmailError, Outcome};
use sm_store::{OpQueue, ReadPool, ReadTx, StoreError};
use tokio::sync::mpsc;

use super::WorkerParts;
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
    /// Wakes the mail ops runner: ops were queued.
    mail_ops: Kick,
    /// Wakes the draft ops runner: ops were queued.
    draft_ops: Kick,
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

impl AccountCtx {
    pub(crate) fn new(parts: WorkerParts, health: Health, queue: FetchQueue) -> Self {
        let WorkerParts {
            deps,
            account,
            id,
            gmail,
            tasks,
            lane,
            sign_out,
        } = parts;
        Self {
            account,
            id,
            gmail,
            queue,
            poll: Kick::new(Arc::clone(tasks.activity())),
            mail_ops: Kick::new(Arc::clone(tasks.activity())),
            draft_ops: Kick::new(Arc::clone(tasks.activity())),
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
        crate::read::read(&self.deps.pool, read).await
    }

    /// The kick that wakes `queue`'s ops runner.
    pub(crate) const fn ops(&self, queue: OpQueue) -> &Kick {
        match queue {
            OpQueue::Mail => &self.mail_ops,
            OpQueue::Drafts => &self.draft_ops,
        }
    }

    /// Records a call's outcome for the account's sync state.
    pub(crate) fn note<T>(&self, result: &Result<T, GmailError>) {
        match result {
            Ok(_) => self.health.succeeded(),
            Err(err) => self.health.failed(err),
        }
    }

    /// Runs a background read's Gmail call until it gets an answer to act on:
    /// with no connection or a transient outcome it waits as SPEC Gmail
    /// errors says and calls again. Any other error is the caller's.
    pub(crate) async fn retried<T, F, Fut>(&self, mut call: F) -> Result<T, GmailError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, GmailError>>,
    {
        let mut backoff = Backoff::new();
        loop {
            let err = match call().await {
                Ok(value) => {
                    self.health.succeeded();
                    return Ok(value);
                }
                Err(err) => err,
            };
            let wait = if *err.outcome() == Outcome::Offline {
                OFFLINE_RETRY
            } else if err.is_transient() {
                backoff.next_delay(&err)
            } else {
                return Err(err);
            };
            self.health.failed(&err);
            tracing::debug!(account = self.account.0, outcome = %err.outcome(), ?wait, "trying again");
            self.deps.network.wait(self.clock(), wait).await;
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
        _ = self.sign_out.send(report);
    }
}
