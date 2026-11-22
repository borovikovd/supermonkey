use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[expect(
    clippy::disallowed_types,
    reason = "the turn is held for the whole run of one op, across its Gmail calls"
)]
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::activity::Tasks;
use crate::drafts::DraftsWriting;
use crate::ops::Landed;
use crate::workers::account::Freshness;
use sm_store::OpQueue;

pub(crate) struct OpsLane {
    #[expect(
        clippy::disallowed_types,
        reason = "the turn is held for the whole run of one op, across its Gmail calls"
    )]
    mail_turn: Arc<Mutex<()>>,
    #[expect(
        clippy::disallowed_types,
        reason = "the turn is held for the whole run of one op, across its Gmail calls"
    )]
    draft_turn: Arc<Mutex<()>>,
    tasks: Tasks,
    pub(crate) landed: Arc<Landed>,
    pub(crate) writing: Arc<DraftsWriting>,
    labels_mapped: AtomicU64,
    pub(crate) label_reads: Freshness,
    pub(crate) send_as_reads: Freshness,
}

impl OpsLane {
    pub(crate) fn new(tasks: Tasks) -> Arc<Self> {
        Arc::new(Self {
            mail_turn: Arc::default(),
            draft_turn: Arc::default(),
            tasks,
            landed: Arc::default(),
            writing: Arc::default(),
            labels_mapped: AtomicU64::new(0),
            label_reads: Freshness::default(),
            send_as_reads: Freshness::default(),
        })
    }

    pub(crate) async fn turn(&self, queue: OpQueue) -> OwnedMutexGuard<()> {
        let turn = match queue {
            OpQueue::Mail => &self.mail_turn,
            OpQueue::Drafts => &self.draft_turn,
        };
        Arc::clone(turn).lock_owned().await
    }

    pub(crate) const fn tasks(&self) -> &Tasks {
        &self.tasks
    }

    pub(crate) fn labels_mapped(&self) -> u64 {
        self.labels_mapped.load(Ordering::SeqCst)
    }

    pub(crate) fn label_mapped(&self) {
        self.labels_mapped.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) async fn stop(&self) {
        self.tasks.stop().await;
    }
}
