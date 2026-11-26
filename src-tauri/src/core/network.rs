use std::sync::Arc;
use std::time::Duration;

use crate::gmail::HttpClient;
use crate::store::{Priority as StorePriority, StoreError, Writer};

use super::events::Effect;
use super::runtime::{Activity, Clock, Kick, Signal, Tasks, WaitOutcome};
use super::sync::AccountManager;
use super::writer_state::WriterState;

const PROBE_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub(crate) struct Network {
    inner: Arc<NetworkInner>,
}

struct NetworkInner {
    online: Signal<bool>,
    probe_url: String,
    http: HttpClient,
    writer: Writer<WriterState>,
    accounts: AccountManager,
    clock: Clock,
    activity: Activity,
    kick: Kick,
}

impl std::fmt::Debug for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Network")
            .field("online", &self.inner.online.get())
            .finish()
    }
}

impl Network {
    #[expect(
        clippy::too_many_arguments,
        reason = "network monitor links signal, HTTP probe, writer broadcast, and account workers"
    )]
    #[must_use]
    pub(crate) fn new(
        online: Signal<bool>,
        gmail_base_url: &str,
        http: HttpClient,
        writer: Writer<WriterState>,
        accounts: AccountManager,
        clock: Clock,
        activity: Activity,
    ) -> Self {
        let probe_url = format!("{}/generate_204", gmail_base_url.trim_end_matches('/'));
        Self {
            inner: Arc::new(NetworkInner {
                online,
                probe_url,
                http,
                writer,
                accounts,
                clock,
                activity,
                kick: Kick::new(),
            }),
        }
    }

    #[must_use]
    pub(crate) fn is_online(&self) -> bool {
        self.inner.online.get()
    }

    pub(crate) async fn set_online(&self, online: bool) {
        let changed = self.inner.online.set(online);
        super::log_err(
            "broadcast online state",
            self.inner
                .writer
                .call(StorePriority::Ui, move |_tx, state| {
                    state.stage_online(online);
                    if online && changed {
                        state.emit(Effect::FetchSubscribedBodies);
                    }
                    Ok::<_, StoreError>(())
                })
                .await,
        );
        if changed {
            if online {
                self.inner.accounts.on_online();
            } else {
                self.inner.accounts.on_offline();
                self.inner.kick.wake(&self.inner.activity);
            }
        }
    }

    pub(crate) fn spawn_probe(&self, tasks: &Tasks) {
        let net = self.clone();
        let cancel = tasks.token();
        tasks.spawn(async move {
            loop {
                if cancel.is_cancelled() {
                    return;
                }
                if net.is_online() {
                    let Some(guard) = net.inner.kick.wait(&net.inner.activity, &cancel).await
                    else {
                        return;
                    };
                    drop(guard);
                    continue;
                }
                let (outcome, guard) = net
                    .inner
                    .kick
                    .wait_timeout(
                        &net.inner.clock,
                        PROBE_INTERVAL,
                        &net.inner.activity,
                        &cancel,
                    )
                    .await;
                if outcome == WaitOutcome::Cancelled {
                    return;
                }
                if outcome == WaitOutcome::Elapsed && !net.is_online() && net.probe_once().await {
                    net.set_online(true).await;
                }
                drop(guard);
            }
        });
    }

    async fn probe_once(&self) -> bool {
        match self
            .inner
            .http
            .google()
            .get(&self.inner.probe_url)
            .send()
            .await
        {
            Ok(resp) => resp.status().as_u16() != 503 && resp.status().as_u16() != 502,
            Err(_) => false,
        }
    }
}
