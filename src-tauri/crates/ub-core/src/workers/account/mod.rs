//! One account's worker (ARCHITECTURE.md, sections 4 and 8.2): a supervised
//! task group under its own cancellation token. Its tasks:
//!
//! - the poller: the connect sync, then `history.list` every 45 s, on
//!   focus, and when kicked, with the full resync when history expired;
//! - the fetchers: the fetch queue's metadata and body reads, in priority
//!   order, and the contacts backfill;
//! - one-off reads: a thread the user opens, a label refresh, a Gmail
//!   listing for a list subscription.
//!
//! They share an [`AccountCtx`] and talk through its kicks, its queue, and
//! the database. Every Gmail result is parsed and sanitized here and written
//! as a small sync job (`ingest`). Stopping cancels the group and waits for
//! it; a slow or failing account never holds up another.

mod contacts;
mod ctx;
mod fetcher;
mod health;
mod history;
mod ingest;
mod poller;
mod profile;
mod queue;
mod read;
mod resync;

use std::sync::Arc;

use ub_domain::ThreadId;
use ub_gmail::Gmail;

pub(crate) use ctx::{AccountCtx, SignOutSender, WorkerDeps, WorkerId};
pub(crate) use fetcher::{fetch_thread, is_current, needs_bodies};
pub(crate) use queue::{LATE, Work};

use self::health::Health;
use self::queue::FetchQueue;
use crate::activity::Tasks;

/// A running account worker.
#[derive(Clone)]
pub(crate) struct AccountWorker {
    ctx: Arc<AccountCtx>,
}

impl AccountWorker {
    /// Starts the worker's tasks in `tasks`, its own group. With `after`, they
    /// start only once that worker, the one it replaces, has stopped, so the
    /// two never poll or replay at once.
    pub(crate) fn spawn(
        deps: Arc<WorkerDeps>,
        account: ub_domain::AccountId,
        id: WorkerId,
        gmail: Gmail,
        tasks: Tasks,
        sign_out: SignOutSender,
        after: Option<Self>,
    ) -> Self {
        let health = Health::new(
            account,
            deps.writer.clone(),
            Arc::clone(&deps.network),
            Arc::clone(&deps.clock),
            tasks.clone(),
        );
        let queue = FetchQueue::new(Arc::clone(tasks.activity()));
        let ctx = Arc::new(AccountCtx::new(
            account, id, gmail, deps, tasks, health, queue, sign_out,
        ));
        let starting = Arc::clone(&ctx);
        ctx.tasks.spawn("account start", async move {
            if let Some(old) = after {
                old.stop().await;
            }
            let tasks = &starting.tasks;
            tasks.spawn("poller", poller::run(Arc::clone(&starting)));
            for _ in 0..fetcher::FETCHERS {
                tasks.spawn("fetcher", fetcher::run(Arc::clone(&starting)));
            }
        });
        Self { ctx }
    }

    /// The state its tasks share, for work started from outside: an opened
    /// thread, a Gmail listing, an attachment download.
    pub(crate) fn ctx(&self) -> &Arc<AccountCtx> {
        &self.ctx
    }

    /// The worker's id.
    pub(crate) fn id(&self) -> WorkerId {
        self.ctx.id
    }

    /// Polls history now.
    pub(crate) fn poll_now(&self) {
        self.ctx.poll.kick();
    }

    /// Reads `sendAs` and the labels again (SPEC Sync: on focus).
    pub(crate) fn refresh_profile(&self) {
        let ctx = Arc::clone(&self.ctx);
        self.ctx.tasks.spawn("refresh profile", async move {
            let refreshed = async {
                profile::refresh_labels(&ctx).await?;
                profile::refresh_send_as(&ctx).await
            }
            .await;
            ctx.note(&refreshed);
        });
    }

    /// Reads the labels again: a message named a label the app does not know.
    pub(crate) fn refresh_labels(&self) {
        let ctx = Arc::clone(&self.ctx);
        self.ctx.tasks.spawn("refresh labels", async move {
            let refreshed = profile::refresh_labels(&ctx).await;
            ctx.note(&refreshed);
        });
    }

    /// Reads an opened or prefetched thread's missing bodies at priority 2.
    pub(crate) fn fetch_opened(&self, thread: ThreadId) {
        let ctx = Arc::clone(&self.ctx);
        self.ctx
            .tasks
            .spawn("fetch thread", fetcher::fetch_opened(ctx, thread));
    }

    /// Cancels every task without waiting.
    pub(crate) fn cancel(&self) {
        self.ctx.tasks.cancel();
    }

    /// Cancels every task and waits until they have ended.
    pub(crate) async fn stop(&self) {
        self.ctx.tasks.stop().await;
    }
}
