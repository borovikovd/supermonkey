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

pub(crate) const OFFLINE_RETRY: Duration = Duration::from_secs(10);

pub(crate) struct WorkerDeps {
    pub(crate) data_dir: PathBuf,
    pub(crate) writer: CoreWriter,
    pub(crate) pool: Arc<ReadPool>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) network: Arc<Network>,
    pub(crate) notifier: Arc<Notifier>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WorkerId(pub(crate) u64);

pub(crate) type SignOutSender = mpsc::UnboundedSender<Tracked<(AccountId, WorkerId)>>;

pub(crate) struct AccountCtx {
    pub(crate) account: AccountId,
    pub(crate) id: WorkerId,
    pub(crate) gmail: Gmail,
    pub(crate) queue: FetchQueue,
    pub(crate) poll: Kick,
    mail_ops: Kick,
    draft_ops: Kick,
    pub(crate) outbox: Kick,
    pub(crate) lane: Arc<OpsLane>,
    pub(crate) health: Health,
    pub(crate) deps: Arc<WorkerDeps>,
    pub(crate) tasks: Tasks,
    sign_out: SignOutSender,
}

fn retry_wait(err: &GmailError, backoff: &mut Backoff) -> Option<Duration> {
    if *err.outcome() == Outcome::Offline {
        Some(OFFLINE_RETRY)
    } else if err.is_transient() {
        Some(backoff.next_delay(err))
    } else {
        None
    }
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

    pub(crate) fn clock(&self) -> &dyn Clock {
        &*self.deps.clock
    }

    pub(crate) fn writer(&self) -> &CoreWriter {
        &self.deps.writer
    }

    pub(crate) async fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce(&ReadTx<'_>) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, Error> {
        crate::read::read(&self.deps.pool, read).await
    }

    pub(crate) const fn ops(&self, queue: OpQueue) -> &Kick {
        match queue {
            OpQueue::Mail => &self.mail_ops,
            OpQueue::Drafts => &self.draft_ops,
        }
    }

    pub(crate) fn note<T>(&self, result: &Result<T, GmailError>) {
        match result {
            Ok(_) => self.health.succeeded(),
            Err(err) => self.health.failed(err),
        }
    }

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
            let Some(wait) = retry_wait(&err, &mut backoff) else {
                return Err(err);
            };
            self.health.failed(&err);
            tracing::debug!(account = self.account.0, outcome = %err.outcome(), ?wait, "trying again");
            self.deps.network.wait(self.clock(), wait).await;
        }
    }

    pub(crate) fn signed_out(&self) {
        tracing::info!(
            account = self.account.0,
            "the grant was refused; signing out"
        );
        let report = self
            .tasks
            .activity()
            .track("sign out", (self.account, self.id));
        _ = self.sign_out.send(report);
    }
}
