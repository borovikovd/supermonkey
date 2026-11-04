//! One account's worker (ARCHITECTURE.md, sections 4 and 8.2): a supervised
//! task group under its own cancellation token. Its tasks:
//!
//! - the poller: the connect sync, then `history.list` every 45 s, on
//!   focus, and when kicked, with the full resync when history expired;
//! - the fetchers: the fetch queue's metadata and body reads, in priority
//!   order, and the contacts backfill;
//! - the ops runner (`workers::ops`): queued Gmail mutations, in order;
//! - the outbox runner (`workers::outbox`): sends, once their undo window
//!   passes;
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
mod opened;
mod poller;
mod profile;
mod queue;
mod read;
mod resync;

use std::sync::Arc;

use ub_domain::{DraftId, ThreadId};
use ub_gmail::Gmail;

pub(crate) use ctx::{AccountCtx, OFFLINE_RETRY, SignOutSender, WorkerDeps, WorkerId};
pub(crate) use fetcher::{fetch_thread, is_current, needs_bodies};
pub(crate) use ingest::store_draft_message;
pub(crate) use queue::OTHER_META;
pub(crate) use queue::{LATE, Work};
pub(crate) use read::{ReadMessage, draft_message, parse};

use self::health::Health;
use self::opened::OpenedReads;
use self::queue::FetchQueue;
use crate::activity::Tasks;
use crate::workers::ops::{self, OpsLane};
use crate::workers::{drafts, outbox};

/// A running account worker.
#[derive(Clone)]
pub(crate) struct AccountWorker {
    ctx: Arc<AccountCtx>,
    /// Reads of conversations' bodies in flight.
    opened: Arc<OpenedReads>,
}

impl AccountWorker {
    /// Starts the worker's tasks in `tasks`, its own group. With `after`, they
    /// start only once that worker, the one it replaces, has stopped, so the
    /// two never poll or replay at once. `lane` is the account's: an op the
    /// old worker had sent finishes there before this one's runner goes on.
    #[expect(
        clippy::too_many_arguments,
        reason = "the worker's parts, built once by the registry"
    )]
    pub(crate) fn spawn(
        deps: Arc<WorkerDeps>,
        account: ub_domain::AccountId,
        id: WorkerId,
        gmail: Gmail,
        tasks: Tasks,
        lane: Arc<OpsLane>,
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
            account, id, gmail, deps, tasks, health, queue, lane, sign_out,
        ));
        let starting = Arc::clone(&ctx);
        ctx.tasks.spawn("account start", async move {
            if let Some(old) = after {
                old.stop().await;
            }
            let tasks = &starting.tasks;
            tasks.spawn("poller", poller::run(Arc::clone(&starting)));
            tasks.spawn("ops", ops::run(Arc::clone(&starting)));
            tasks.spawn("outbox", outbox::run(Arc::clone(&starting)));
            for _ in 0..fetcher::FETCHERS {
                tasks.spawn("fetcher", fetcher::run(Arc::clone(&starting)));
            }
        });
        Self {
            ctx,
            opened: Arc::default(),
        }
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

    /// Cancels every task and waits until they have ended.
    pub(crate) async fn stop(&self) {
        self.ctx.tasks.stop().await;
    }
}
