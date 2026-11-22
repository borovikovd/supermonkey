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

const QUIET_STATUS_AFTER: Duration = Duration::from_secs(30);

pub(crate) struct Health {
    account: AccountId,
    writer: CoreWriter,
    network: Arc<Network>,
    clock: Arc<dyn Clock>,
    tasks: Tasks,
    state: Arc<Mutex<State>>,
}

struct State {
    status: Option<SyncStatus>,
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

    pub(crate) fn succeeded(&self) {
        self.network.set_online(true);
        self.state.lock().retrying = None;
        self.show(SyncStatus::Idle);
    }

    pub(crate) fn syncing(&self) {
        self.show(SyncStatus::Syncing);
    }

    pub(crate) fn failed(&self, err: &GmailError) {
        let outcome = err.outcome();
        if matches!(outcome, Outcome::Offline) {
            self.network.set_online(false);
            self.show(SyncStatus::Offline);
        } else if outcome.is_transient() {
            self.retrying();
        }
    }

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
