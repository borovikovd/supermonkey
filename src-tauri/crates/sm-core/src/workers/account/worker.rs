use std::sync::Arc;

use parking_lot::Mutex;
use sm_domain::{AccountId, DraftId, ThreadId};
use sm_gmail::Gmail;
use sm_store::OpQueue;

use super::ctx::{AccountCtx, SignOutSender, WorkerDeps, WorkerId};
use super::health::Health;
use super::queue::FetchQueue;
use super::{fetcher, poller, profile};
use crate::activity::Tasks;
use crate::workers::grants::Grant;
use crate::workers::ops::{self, OpsLane};
use crate::workers::runs::Runs;
use crate::workers::{drafts, outbox};

pub(crate) struct WorkerParts {
    pub(crate) deps: Arc<WorkerDeps>,
    pub(crate) account: AccountId,
    pub(crate) id: WorkerId,
    pub(crate) gmail: Gmail,
    pub(crate) tasks: Tasks,
    pub(crate) lane: Arc<OpsLane>,
    pub(crate) sign_out: SignOutSender,
}

#[derive(Clone)]
pub(crate) struct AccountWorker {
    ctx: Arc<AccountCtx>,
    opened: Arc<Runs<ThreadId>>,
    grant: Grant,
    replaced: Arc<Mutex<Option<Self>>>,
}

impl AccountWorker {
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

    pub(crate) fn start(self, replaced: Option<Self>) {
        self.replaced.lock().clone_from(&replaced);
        let ctx = Arc::clone(&self.ctx);
        ctx.tasks.spawn("account start", async move {
            if let Some(old) = replaced {
                old.stop().await;
                self.replaced.lock().take();
            }
            let (ctx, tasks) = (&self.ctx, &self.ctx.tasks);
            tasks.spawn("poller", poller::run(Arc::clone(ctx)));
            for queue in OpQueue::ALL {
                tasks.spawn("ops", ops::run(Arc::clone(ctx), queue));
            }
            tasks.spawn("outbox", outbox::run(Arc::clone(ctx)));
            for _ in 0..fetcher::FETCHERS {
                tasks.spawn("fetcher", fetcher::run(Arc::clone(ctx)));
            }
        });
    }

    pub(crate) const fn ctx(&self) -> &Arc<AccountCtx> {
        &self.ctx
    }

    pub(crate) fn id(&self) -> WorkerId {
        self.ctx.id
    }

    pub(crate) const fn grant(&self) -> Grant {
        self.grant
    }

    pub(crate) fn poll_now(&self) {
        self.ctx.poll.kick();
    }

    pub(crate) fn kick_ops(&self) {
        for queue in OpQueue::ALL {
            self.ctx.ops(queue).kick();
        }
    }

    pub(crate) fn kick_outbox(&self) {
        self.ctx.outbox.kick();
    }

    pub(crate) fn download_parts(&self, draft: DraftId) {
        let ctx = Arc::clone(&self.ctx);
        self.ctx.tasks.spawn("draft parts", async move {
            if let Err(e) = drafts::download_parts(&ctx, draft).await {
                tracing::info!(account = ctx.account.0, error = %e, "couldn't download a draft's parts yet");
            }
        });
    }

    pub(crate) fn read_draft(&self, draft: DraftId) {
        let ctx = Arc::clone(&self.ctx);
        self.ctx.tasks.spawn("read draft", async move {
            drafts::read_skipped(&ctx, draft).await;
        });
    }

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

    pub(crate) fn refresh_labels(&self) {
        let ctx = Arc::clone(&self.ctx);
        self.ctx.tasks.spawn("refresh labels", async move {
            let refreshed = profile::refresh_labels(&ctx).await;
            ctx.note(&refreshed);
        });
    }

    pub(crate) fn fetch_opened(&self, thread: ThreadId) {
        let Some(read) = self.opened.begin(thread) else {
            return;
        };
        let ctx = Arc::clone(&self.ctx);
        self.ctx.tasks.spawn("fetch thread", async move {
            tokio::select! {
                biased;
                () = read.stopped().cancelled() => {}
                () = fetcher::fetch_opened(&ctx, thread) => {}
            }
            drop(read);
        });
    }

    pub(crate) fn release_opened(&self, thread: ThreadId) {
        self.opened.stop(&thread);
    }

    pub(crate) fn cancel(&self) {
        self.ctx.tasks.cancel();
    }

    pub(crate) async fn stop(&self) {
        self.ctx.tasks.stop().await;
        let replaced = self.replaced.lock().take();
        if let Some(old) = replaced {
            Box::pin(old.stop()).await;
        }
    }
}
