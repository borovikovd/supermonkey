//! The outbox runner. Filled in with the composer.

use std::sync::Arc;

use super::{AccountRt, EngineRef};

pub async fn run(_e: EngineRef, rt: Arc<AccountRt>) {
    while !rt.is_stopped() {
        super::wait(&rt.outbox_kick, 60_000).await;
    }
}

pub fn on_launch(_e: &EngineRef) {}
