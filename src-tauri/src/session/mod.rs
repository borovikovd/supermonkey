#[cfg(feature = "web")]
pub mod web;

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::core::{ChannelSink, Core, CoreError};
use crate::domain::{AccountId, ThreadId};
use crate::ipc::{AppEvent, Command, CommandError, ErrorKind, LaunchInfo, Reply};

const WINDOW_SHOW_DEADLINE: Duration = Duration::from_secs(5);
const QUIT_DEADLINE: Duration = Duration::from_secs(10);

pub trait Host: Send + Sync {
    fn show_window(&self);
    fn emit(&self, event: AppEvent);
    fn exit(&self);
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum QuitPhase {
    #[default]
    Running,
    Quitting,
    Exited,
}

#[derive(Default)]
struct State {
    shown: bool,
    connected: bool,
    quit: QuitPhase,
}

struct SessionInner {
    handle: tokio::runtime::Handle,
    core: Arc<Core>,
    host: Box<dyn Host>,
    state: Mutex<State>,
    show_cancel: CancellationToken,
    quit_cancel: CancellationToken,
    exited_notify: Notify,
}

#[derive(Clone)]
pub struct Session {
    inner: Arc<SessionInner>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

impl Session {
    #[must_use]
    pub fn new(core: Arc<Core>, host: Box<dyn Host>) -> Self {
        let handle = tokio::runtime::Handle::current();
        let session = Self {
            inner: Arc::new(SessionInner {
                handle,
                core,
                host,
                state: Mutex::new(State::default()),
                show_cancel: CancellationToken::new(),
                quit_cancel: CancellationToken::new(),
                exited_notify: Notify::new(),
            }),
        };

        let weak = Arc::downgrade(&session.inner);
        let clock = session.inner.core.clock().clone();
        let cancel = session.inner.show_cancel.clone();
        session.inner.handle.spawn(async move {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {}
                () = clock.sleep(WINDOW_SHOW_DEADLINE) => {
                    if let Some(inner) = weak.upgrade() {
                        let s = Self { inner };
                        s.show_window_once();
                    }
                }
            }
        });

        session
    }

    #[must_use]
    pub fn core(&self) -> &Core {
        &self.inner.core
    }

    pub fn connect(&self, sink: ChannelSink) -> Result<LaunchInfo, CommandError> {
        self.inner.core.set_channel(Some(sink));
        self.mark_connected_and_show();
        self.inner
            .core
            .connect()
            .map_err(CoreError::into_command_error)
    }

    pub fn mark_connected_and_show(&self) {
        let should_show = {
            let mut st = self.inner.state.lock();
            st.connected = true;
            if st.shown {
                false
            } else {
                st.shown = true;
                true
            }
        };
        if should_show {
            self.inner.show_cancel.cancel();
            self.inner.host.show_window();
        }
    }

    fn show_window_once(&self) {
        let should_show = {
            let mut st = self.inner.state.lock();
            if st.shown || st.quit == QuitPhase::Exited {
                false
            } else {
                st.shown = true;
                true
            }
        };
        if should_show {
            self.inner.show_cancel.cancel();
            self.inner.host.show_window();
        }
    }

    #[must_use]
    pub fn focus(&self, focused: bool) -> Option<(AccountId, Option<ThreadId>)> {
        let target = self.inner.core.on_focus(focused);
        if let Some((account, thread)) = target {
            self.inner
                .host
                .emit(AppEvent::NotificationOpen { account, thread });
        }
        target
    }

    pub fn request_quit(&self) {
        let connected = {
            let mut st = self.inner.state.lock();
            if st.quit != QuitPhase::Running {
                return;
            }
            st.quit = QuitPhase::Quitting;
            st.connected
        };

        if !connected {
            let this = self.clone();
            self.inner.handle.spawn(async move {
                this.finish_exit(true).await;
            });
            return;
        }

        self.inner.host.emit(AppEvent::QuitRequested);

        let this = self.clone();
        let clock = self.inner.core.clock().clone();
        let cancel = self.inner.quit_cancel.clone();
        self.inner.handle.spawn(async move {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {}
                () = clock.sleep(QUIT_DEADLINE) => {
                    this.finish_exit(true).await;
                }
            }
        });
    }

    pub fn quit_now(&self) {
        let this = self.clone();
        self.inner.handle.spawn(async move {
            this.finish_exit(true).await;
        });
    }

    pub async fn terminate(&self) {
        self.finish_exit(false).await;
    }

    pub async fn wait_exited(&self) {
        loop {
            let notified = self.inner.exited_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inner.state.lock().quit == QuitPhase::Exited {
                return;
            }
            notified.await;
        }
    }

    async fn finish_exit(&self, notify_host: bool) {
        let should_run = {
            let mut st = self.inner.state.lock();
            if st.quit == QuitPhase::Exited {
                false
            } else {
                st.quit = QuitPhase::Exited;
                true
            }
        };
        if !should_run {
            return;
        }
        let _busy = self.inner.core.enter_busy();
        self.inner.show_cancel.cancel();
        self.inner.quit_cancel.cancel();
        self.inner.core.stop().await;
        if notify_host {
            self.inner.host.exit();
        }
        self.inner.exited_notify.notify_waiters();
    }

    #[expect(
        clippy::too_many_lines,
        reason = "central command dispatcher maps every IPC Command variant to its Core method"
    )]
    pub async fn dispatch(&self, name: &str, args: Value) -> Result<Reply, CommandError> {
        let cmd = Command::parse(name, args).map_err(|message| CommandError {
            kind: ErrorKind::Other,
            message,
        })?;
        let core = &self.inner.core;
        let encoded = match cmd {
            Command::Connect { reply } => {
                self.mark_connected_and_show();
                let info = core.connect().map_err(CoreError::into_command_error)?;
                reply.send(info)
            }
            Command::Submit { actions, reply } => {
                core.submit(actions)
                    .await
                    .map_err(CoreError::into_command_error)?;
                reply.send(())
            }
            Command::FetchRows {
                drafts,
                keys,
                reply,
            } => {
                let rows = core
                    .fetch_rows(drafts, &keys)
                    .map_err(CoreError::into_command_error)?;
                reply.send(rows)
            }
            Command::GetBodies { ids, reply } => {
                let bodies = core
                    .get_bodies(&ids)
                    .map_err(CoreError::into_command_error)?;
                reply.send(bodies)
            }
            Command::Autocomplete {
                account,
                query,
                reply,
            } => {
                let items = core
                    .autocomplete(account, &query)
                    .map_err(CoreError::into_command_error)?;
                reply.send(items)
            }
            Command::AddAccount { reply } => {
                let id = core
                    .add_account()
                    .await
                    .map_err(CoreError::into_command_error)?;
                reply.send(id)
            }
            Command::SignInAgain { account, reply } => {
                core.sign_in_again(account)
                    .await
                    .map_err(CoreError::into_command_error)?;
                reply.send(())
            }
            Command::OpenAttachment { attachment, reply } => {
                core.open_attachment(attachment)
                    .await
                    .map_err(CoreError::into_command_error)?;
                reply.send(())
            }
            Command::OpenUrl { url, reply } => {
                core.open_url(&url).map_err(CoreError::into_command_error)?;
                reply.send(())
            }
            Command::Unsubscribe {
                account,
                thread,
                reply,
            } => {
                let res = core
                    .unsubscribe(account, thread)
                    .await
                    .map_err(CoreError::into_command_error)?;
                reply.send(res)
            }
            Command::Block {
                account,
                thread,
                domain,
                reply,
            } => {
                let res = core
                    .block(account, thread, domain)
                    .await
                    .map_err(CoreError::into_command_error)?;
                reply.send(res)
            }
            Command::BlockedSenders { account, reply } => {
                let res = core
                    .blocked_senders(account)
                    .await
                    .map_err(CoreError::into_command_error)?;
                reply.send(res)
            }
            Command::Unblock { account, id, reply } => {
                core.unblock(account, &id)
                    .await
                    .map_err(CoreError::into_command_error)?;
                reply.send(())
            }
            Command::SenderThreads {
                account,
                thread,
                reply,
            } => {
                let res = core
                    .sender_threads(account, thread)
                    .map_err(CoreError::into_command_error)?;
                reply.send(res)
            }
            Command::AddAttachment {
                draft,
                filename,
                mime_type,
                data,
                reply,
            } => {
                let res = core
                    .add_attachment(draft, filename, mime_type, &data)
                    .await
                    .map_err(CoreError::into_command_error)?;
                reply.send(res)
            }
            Command::QuitNow { reply } => {
                self.quit_now();
                reply.send(())
            }
            Command::PerfReport {
                action,
                ms,
                budget,
                reply,
            } => {
                log_perf(&action, ms, budget);
                reply.send(())
            }
            Command::SnoozeOptions { text, reply } => {
                let choices = core.snooze_options(&text);
                reply.send(choices)
            }
        };
        encoded.map_err(|err| CoreError::internal(err.to_string()).into_command_error())
    }
}

fn log_perf(action: &str, ms: f64, budget: f64) {
    if ms > budget {
        tracing::warn!(%action, ms, budget, "ui action exceeded perf budget");
    }
}
