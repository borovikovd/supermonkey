//! The account worker handle: built from its parts, started after the
//! worker it replaces has stopped, and what the core asks of it.

use std::sync::Arc;

use parking_lot::Mutex;
use ub_domain::{AccountId, DraftId, ThreadId};
use ub_gmail::Gmail;

use super::ctx::{AccountCtx, SignOutSender, WorkerDeps, WorkerId};
use super::health::Health;
use super::opened::OpenedReads;
use super::queue::FetchQueue;
use super::{fetcher, poller, profile};
use crate::activity::Tasks;
use crate::workers::grants::Grant;
use crate::workers::ops::{self, OpsLane};
use crate::workers::{drafts, outbox};

/// What the registry builds a worker from.
pub(crate) struct WorkerParts {
    pub(crate) deps: Arc<WorkerDeps>,
    pub(crate) account: AccountId,
    pub(crate) id: WorkerId,
    /// The account's Gmail client, on the grant the worker runs on.
    pub(crate) gmail: Gmail,
    /// The worker's own task group.
    pub(crate) tasks: Tasks,
    /// The account's op lane: an op an older worker had sent finishes there
    /// before this one's runner goes on.
    pub(crate) lane: Arc<OpsLane>,
    pub(crate) sign_out: SignOutSender,
}

/// A running account worker.
#[derive(Clone)]
pub(crate) struct AccountWorker {
    ctx: Arc<AccountCtx>,
    /// Reads of conversations' bodies in flight.
    opened: Arc<OpenedReads>,
    /// The sign-in whose grant it runs on.
    grant: Grant,
    /// The worker it replaced, until that one has stopped: stopping this
    /// one waits for it too, so no two of an account's workers ever run at
    /// once, however quickly they replace each other.
    replaced: Arc<Mutex<Option<AccountWorker>>>,
}

impl AccountWorker {
    /// A worker on `grant` whose tasks have not started.
    pub(crate) fn new(parts: WorkerParts, grant: Grant) -> Self {
        let health = Health::new(
            parts.account,
            parts.deps.writer.clone(),
            Arc::clone(&parts.deps.network),
            Arc::clone(&parts.deps.clock),
            parts.tasks.clone(),
        );
        let queue = FetchQueue::new(Arc::clone(parts.tasks.activity()));
        Self {
            ctx: Arc::new(AccountCtx::new(parts, health, queue)),
            opened: Arc::default(),
            grant,
            replaced: Arc::default(),
        }
    }

    /// Starts the worker's tasks. With `replaced`, they start only once that
    /// worker has stopped, so the two never poll or replay at once.
    pub(crate) fn start(&self, replaced: Option<Self>) {
        self.replaced.lock().clone_from(&replaced);
        let starting = self.clone();
        self.ctx.tasks.spawn("account start", async move {
            if let Some(old) = replaced {
                old.stop().await;
                starting.replaced.lock().take();
            }
            let (ctx, tasks) = (&starting.ctx, &starting.ctx.tasks);
            tasks.spawn("poller", poller::run(Arc::clone(ctx)));
            tasks.spawn("ops", ops::run(Arc::clone(ctx)));
            tasks.spawn("outbox", outbox::run(Arc::clone(ctx)));
            for _ in 0..fetcher::FETCHERS {
                tasks.spawn("fetcher", fetcher::run(Arc::clone(ctx)));
            }
        });
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

    /// The sign-in whose grant it runs on.
    pub(crate) fn grant(&self) -> Grant {
        self.grant
    }

    /// Polls history now.
    pub(crate) fn poll_now(&self) {
        self.ctx.poll.kick();
    }

    /// Wakes the ops runner: ops were queued.
    pub(crate) fn kick_ops(&self) {
        self.ctx.ops.kick();
    }

    /// Wakes the outbox runner: a send was queued or undone.
    pub(crate) fn kick_outbox(&self) {
        self.ctx.outbox.kick();
    }

    /// Downloads the parts of a draft read from Gmail, now open in a
    /// composer (SPEC Addressing).
    pub(crate) fn download_parts(&self, draft: DraftId) {
        let ctx = Arc::clone(&self.ctx);
        self.ctx.tasks.spawn("draft parts", async move {
            if let Err(e) = drafts::download_parts(&ctx, &draft).await {
                tracing::info!(account = ctx.account.0, error = %e, "couldn't download a draft's parts yet");
            }
        });
    }

    /// Reads a Gmail change to a draft that was skipped while a composer
    /// had it open (SPEC Drafts).
    pub(crate) fn read_draft(&self, draft: DraftId) {
        let ctx = Arc::clone(&self.ctx);
        self.ctx.tasks.spawn("read draft", async move {
            drafts::read_skipped(&ctx, &draft).await;
        });
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

    /// Reads an opened or prefetched thread's missing bodies at priority 2,
    /// unless a read of them is in flight already. The read stops once no
    /// subscription shows the thread ([`AccountWorker::release_opened`]).
    pub(crate) fn fetch_opened(&self, thread: ThreadId) {
        let Some(read) = self.opened.begin(thread) else {
            return;
        };
        let ctx = Arc::clone(&self.ctx);
        self.ctx.tasks.spawn("fetch thread", async move {
            tokio::select! {
                biased;
                () = read.stopped().cancelled() => {}
                () = fetcher::fetch_opened(ctx, thread) => {}
            }
            drop(read);
        });
    }

    /// Stops the read of `thread`'s bodies for a conversation: no
    /// subscription shows it any more.
    pub(crate) fn release_opened(&self, thread: ThreadId) {
        self.opened.stop(thread);
    }

    /// Cancels every task without waiting.
    pub(crate) fn cancel(&self) {
        self.ctx.tasks.cancel();
    }

    /// Cancels every task and waits until they have ended, and until the
    /// worker it replaced has stopped too.
    pub(crate) async fn stop(&self) {
        self.ctx.tasks.stop().await;
        let replaced = self.replaced.lock().take();
        if let Some(old) = replaced {
            Box::pin(old.stop()).await;
        }
    }
}
