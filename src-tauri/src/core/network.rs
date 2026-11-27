use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::gmail::{GmailError, HttpClient, Outcome};
use crate::store::{Priority as StorePriority, StoreError, Writer};

use super::events::Effect;
use super::runtime::{Activity, Clock, Kick, Signal, Tasks, WaitOutcome};
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
    #[must_use]
    pub(crate) fn new(
        gmail_base_url: &str,
        http: HttpClient,
        writer: Writer<WriterState>,
        clock: Clock,
        activity: Activity,
    ) -> Self {
        let probe_url = format!("{}/generate_204", gmail_base_url.trim_end_matches('/'));
        Self {
            inner: Arc::new(NetworkInner {
                online: Signal::new(true),
                probe_url,
                http,
                writer,
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

    #[must_use]
    pub(crate) fn subscribe(&self) -> watch::Receiver<bool> {
        self.inner.online.subscribe()
    }

    pub(crate) async fn set_online(&self, online: bool) {
        if !self.inner.online.set(online) {
            return;
        }
        super::log_err(
            "broadcast online state",
            self.inner
                .writer
                .call(StorePriority::Ui, move |_tx, state| {
                    state.stage_online(online);
                    state.emit(Effect::NetworkChanged { online });
                    Ok::<_, StoreError>(())
                })
                .await,
        );
        if !online {
            self.inner.kick.wake(&self.inner.activity);
        }
    }

    pub(crate) async fn lost_if_unreachable(&self, err: &GmailError) {
        if err.outcome == Outcome::Offline {
            self.set_online(false).await;
        }
    }

    #[cfg(any(test, feature = "fake"))]
    pub(crate) async fn probe_now(&self) {
        let reachable = self.probe_once().await;
        self.set_online(reachable).await;
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
