//! An account's sync state as the rail shows it (SPEC Gmail errors): offline,
//! and the quiet "Can't reach Gmail" after 30 s of retrying.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use sm_domain::{AccountId, SyncStatus};
use sm_gmail::{GmailError, Outcome};
use sm_store::Priority;
use tokio_util::sync::CancellationToken;

use crate::activity::Tasks;
use crate::clock::{Clock, sleep};
use crate::workers::network::Network;
use crate::writer::CoreWriter;

/// How long Gmail errors go on before the account shows a quiet sync status.
const QUIET_STATUS_AFTER: Duration = Duration::from_secs(30);

/// Records what each Gmail call of one account ended in.
pub(crate) struct Health {
    account: AccountId,
    writer: CoreWriter,
    network: Arc<Network>,
    clock: Arc<dyn Clock>,
    tasks: Tasks,
    state: Arc<Mutex<State>>,
}

struct State {
    /// The status last asked of the writer; none yet, so the first is
    /// written whatever the stored one is (it may be from before a launch).
    status: Option<SyncStatus>,
    /// The retrying episode under way, if any; a success ends it.
    retrying: Option<u64>,
    episodes: u64,
}

impl Health {
    pub(crate) fn new(
        account: AccountId,
        writer: CoreWriter,
        network: Arc<Network>,
        clock: Arc<dyn Clock>,
        tasks: Tasks,
    ) -> Self {
        Self {
            account,
            writer,
            network,
            clock,
            tasks,
            state: Arc::new(Mutex::new(State {
                status: None,
                retrying: None,
                episodes: 0,
            })),
        }
    }

    /// A Gmail call answered.
    pub(crate) fn succeeded(&self) {
        self.network.set_online(true);
        self.state.lock().retrying = None;
        self.show(SyncStatus::Idle);
    }

    /// The connect sync started.
    pub(crate) fn syncing(&self) {
        self.show(SyncStatus::Syncing);
    }

    /// A Gmail call failed with `err`.
    pub(crate) fn failed(&self, err: &GmailError) {
        match err.outcome() {
            Outcome::Offline => {
                self.network.set_online(false);
                self.show(SyncStatus::Offline);
            }
            outcome if outcome.is_transient() => self.retrying(),
            _ => {}
        }
    }

    /// The first transient failure since the last success starts the clock
    /// on the quiet status.
    fn retrying(&self) {
        let episode = {
            let mut state = self.state.lock();
            if state.retrying.is_some() {
                return;
            }
            state.episodes = state.episodes.saturating_add(1);
            state.retrying = Some(state.episodes);
            state.episodes
        };
        let clock = Arc::clone(&self.clock);
        let state = Arc::clone(&self.state);
        let writer = self.writer.clone();
        let (account, cancel) = (self.account, self.tasks.token().clone());
        self.tasks.spawn("sync status", async move {
            sleep(&*clock, QUIET_STATUS_AFTER).await;
            let still = state.lock().retrying == Some(episode);
            if still {
                show(&state, &writer, account, &cancel, SyncStatus::Retrying);
            }
        });
    }

    fn show(&self, status: SyncStatus) {
        show(
            &self.state,
            &self.writer,
            self.account,
            self.tasks.token(),
            status,
        );
    }
}

/// Asks the writer for `status`, if it is not the one asked for last. The job
/// reads the status when it runs, so racing changes end on the last one. A
/// stopped worker writes nothing: the account may have signed in again since.
fn show(
    state: &Arc<Mutex<State>>,
    writer: &CoreWriter,
    account: AccountId,
    cancel: &CancellationToken,
    status: SyncStatus,
) {
    {
        let mut s = state.lock();
        if s.status == Some(status) {
            return;
        }
        s.status = Some(status);
    }
    let (state, cancel) = (Arc::clone(state), cancel.clone());
    writer.queue(Priority::Sync, "store a sync status", move |tx, _| {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let Some(status) = state.lock().status else {
            return Ok(());
        };
        tx.accounts().set_sync_status(account, status).map(drop)
    });
}
