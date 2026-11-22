use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;
use sm_core::activity::{Activity, Tasks};
use sm_core::clock::{self, Clock};
use sm_core::{App, Error, Notification};
use sm_ipc::{AppEvent, ChannelMessage, LaunchInfo};
use tokio::runtime::Handle;
use tokio::sync::OnceCell;

use crate::commands::CmdResult;
use crate::{Answer, dispatch};

const SHOW_DEADLINE: Duration = Duration::from_secs(5);

const PAGE_QUIT_DEADLINE: Duration = Duration::from_secs(10);

pub trait Host: Send + Sync {
    fn emit(&self, event: &AppEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    fn show_window(&self);

    fn exit(&self);
}

pub struct Session {
    core: Arc<App>,
    host: Box<dyn Host>,
    clock: Arc<dyn Clock>,
    work: Tasks,
    quitter: Tasks,
    connected: AtomicBool,
    quitting: AtomicBool,
    stopped: OnceCell<()>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("connected", &self.connected)
            .field("quitting", &self.quitting)
            .finish_non_exhaustive()
    }
}

impl Session {
    #[must_use]
    pub fn start(
        core: Arc<App>,
        host: Box<dyn Host>,
        clock: Arc<dyn Clock>,
        runtime: &Handle,
        activity: &Arc<Activity>,
    ) -> Arc<Self> {
        core.lifecycle().launch();
        let group = || Tasks::new(runtime.clone(), Arc::clone(activity));
        let session = Arc::new(Self {
            core,
            host,
            clock,
            work: group(),
            quitter: group(),
            connected: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
            stopped: OnceCell::new(),
        });
        session.show_after_deadline();
        session
    }

    #[must_use]
    pub const fn core(&self) -> &Arc<App> {
        &self.core
    }

    #[must_use]
    pub const fn tasks(&self) -> &Tasks {
        &self.work
    }

    pub async fn connect(
        &self,
        sink: Box<dyn FnMut(ChannelMessage) + Send>,
    ) -> Result<LaunchInfo, Error> {
        let info = self.core.lifecycle().connect(sink).await?;
        self.connected.store(true, Ordering::Release);
        self.host.show_window();
        Ok(info)
    }

    pub async fn dispatch(self: &Arc<Self>, command: &str, args: Value) -> CmdResult<Value> {
        match dispatch(&self.core, command, args).await? {
            Answer::Reply(reply) => Ok(reply.into_json()),
            Answer::Quit => {
                self.quit();
                Ok(Value::Null)
            }
        }
    }

    pub fn focus(&self, focused: bool) -> Option<Notification> {
        let clicked = self.core.lifecycle().focus(focused)?;
        let event = AppEvent::NotificationOpen {
            account: clicked.account,
            thread: clicked.thread,
        };
        if let Err(e) = self.host.emit(&event) {
            tracing::warn!(error = %e, "couldn't open a clicked notification");
        }
        Some(clicked)
    }

    pub fn request_quit(self: &Arc<Self>) {
        if self.quitting.load(Ordering::Acquire) {
            return;
        }
        if !self.connected.load(Ordering::Acquire) {
            self.quit();
            return;
        }
        self.quit_after_deadline();
        if let Err(e) = self.host.emit(&AppEvent::QuitRequested) {
            tracing::warn!(error = %e, "couldn't ask the page to quit; quitting");
            self.quit();
        }
    }

    pub fn quit(self: &Arc<Self>) {
        if self.quitting.swap(true, Ordering::AcqRel) {
            return;
        }
        let session = Arc::clone(self);
        self.quitter.spawn("quit", async move {
            session.stop().await;
            session.host.exit();
        });
    }

    pub async fn terminated(&self) {
        self.quitting.store(true, Ordering::Release);
        self.stop().await;
    }

    async fn stop(&self) {
        self.stopped
            .get_or_init(|| async {
                self.work.stop().await;
                self.core.lifecycle().quit().await;
            })
            .await;
    }

    fn show_after_deadline(self: &Arc<Self>) {
        let session = Arc::clone(self);
        self.work.spawn("show deadline", async move {
            clock::sleep(&*session.clock, SHOW_DEADLINE).await;
            if !session.connected.load(Ordering::Acquire) {
                tracing::error!("the page did not connect to the core; showing the window anyway");
                session.host.show_window();
            }
        });
    }

    fn quit_after_deadline(self: &Arc<Self>) {
        let session = Arc::clone(self);
        self.work.spawn("quit deadline", async move {
            clock::sleep(&*session.clock, PAGE_QUIT_DEADLINE).await;
            if !session.quitting.load(Ordering::Acquire) {
                tracing::warn!("the page did not answer the quit in time; quitting");
                session.quit();
            }
        });
    }
}
