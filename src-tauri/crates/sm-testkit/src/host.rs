use std::sync::Arc;

use sm_bridge::{Host, Pages};
use sm_ipc::AppEvent;
use tokio::sync::Notify;

use crate::platform::RecordingPlatform;

pub(crate) struct TestHost {
    pub(crate) pages: Pages,
    pub(crate) platform: Arc<RecordingPlatform>,
    pub(crate) exited: Arc<Notify>,
}

impl Host for TestHost {
    fn emit(&self, event: &AppEvent) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.pages.emit(event)?;
        self.platform.emitted(event.clone());
        Ok(())
    }

    fn show_window(&self) {
        self.platform.window_shown();
    }

    fn exit(&self) {
        self.platform.exited();
        self.exited.notify_one();
    }
}
