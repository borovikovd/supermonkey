use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sm_store::Priority;

use crate::activity::{Activity, Signal};
use crate::clock::Clock;
use crate::writer::CoreWriter;

pub(crate) struct Network {
    online: Arc<AtomicBool>,
    writer: CoreWriter,
    back: Signal,
    lost: Signal,
}

impl Network {
    pub(crate) fn new(writer: CoreWriter, activity: Arc<Activity>) -> Self {
        Self {
            online: Arc::new(AtomicBool::new(true)),
            writer,
            back: Signal::new(Arc::clone(&activity)),
            lost: Signal::new(activity),
        }
    }

    pub(crate) fn is_online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }

    pub(crate) fn set_online(&self, online: bool) {
        if self.online.swap(online, Ordering::SeqCst) == online {
            return;
        }
        tracing::info!(online, "Gmail reachability changed");
        let flag = Arc::clone(&self.online);
        self.writer
            .queue(Priority::Ui, "record the network state", move |_, state| {
                state.set_online(flag.load(Ordering::SeqCst));
                Ok(())
            });
        if online {
            self.back.fire();
        } else {
            self.lost.fire();
        }
    }

    pub(crate) fn retry_now(&self) {
        self.back.fire();
    }

    pub(crate) async fn wait(&self, clock: &dyn Clock, timeout: Duration) {
        self.back.wait(clock, timeout).await;
    }

    pub(crate) async fn until_lost(&self, clock: &dyn Clock, timeout: Duration) {
        self.lost
            .wait_unless(clock, timeout, || !self.is_online())
            .await;
    }

    pub(crate) async fn until_online(&self, clock: &dyn Clock, retry: Duration) {
        while !self.is_online() {
            self.back
                .wait_unless(clock, retry, || self.is_online())
                .await;
        }
    }
}
