//! What one account's op replay, draft writes, and profile reads keep when
//! its worker is replaced.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::activity::Tasks;
use crate::drafts::DraftsWriting;
use crate::ops::Landed;
use crate::workers::account::Freshness;

/// The account's op lane: one op runs at a time, and a run outlives the
/// worker that started it.
///
/// Replacing a worker (a second sign-in, ACC-13) cancels the old one's
/// tasks. If that cancelled an op whose request Gmail already had, the new
/// worker would send it again. So a run happens in the lane's own task
/// group, holding the lane's turn from reading the op until its result is
/// written, and the new worker's runner takes the turn first: it waits for
/// the old run to land and then sees the op gone.
pub(crate) struct OpsLane {
    /// Held for the whole run of one op, across its Gmail calls: an async
    /// mutex, because holding it across `.await` is its purpose.
    turn: Arc<Mutex<()>>,
    /// Op runs: cancelled only when the account goes or the app quits.
    tasks: Tasks,
    /// Ops that landed while reads were in flight.
    pub(crate) landed: Arc<Landed>,
    /// The account's drafts being written to Gmail, by an upsert or the
    /// outbox: a write outlives a replaced worker too.
    pub(crate) writing: Arc<DraftsWriting>,
    /// How many labels made here got their Gmail id: a label list read
    /// before one did cannot know it yet.
    labels_mapped: AtomicU64,
    /// Orders the label list reads' answers.
    pub(crate) label_reads: Freshness,
    /// Orders the `sendAs` reads' answers.
    pub(crate) send_as_reads: Freshness,
}

impl OpsLane {
    /// A lane whose runs belong to `tasks`.
    pub(crate) fn new(tasks: Tasks) -> Arc<Self> {
        Arc::new(Self {
            turn: Arc::default(),
            tasks,
            landed: Arc::default(),
            writing: Arc::default(),
            labels_mapped: AtomicU64::new(0),
            label_reads: Freshness::default(),
            send_as_reads: Freshness::default(),
        })
    }

    /// Waits for the lane's turn: no op of the account is running.
    pub(crate) async fn turn(&self) -> OwnedMutexGuard<()> {
        Arc::clone(&self.turn).lock_owned().await
    }

    /// The group op runs belong to.
    pub(crate) fn tasks(&self) -> &Tasks {
        &self.tasks
    }

    /// How many labels made here have got their Gmail id so far.
    pub(crate) fn labels_mapped(&self) -> u64 {
        self.labels_mapped.load(Ordering::SeqCst)
    }

    /// A label made here got its Gmail id. Call it in the writer job that
    /// stores the id, so label list writes are ordered with it.
    pub(crate) fn label_mapped(&self) {
        self.labels_mapped.fetch_add(1, Ordering::SeqCst);
    }

    /// Cancels the run in flight, if any, and waits for it to end: the
    /// account is gone.
    pub(crate) async fn stop(&self) {
        self.tasks.stop().await;
    }
}
