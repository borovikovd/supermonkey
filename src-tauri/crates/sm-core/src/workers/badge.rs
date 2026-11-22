use std::sync::Arc;
use std::time::Duration;

use sm_store::ReadPool;

use crate::Platform;
use crate::activity::Kick;
use crate::clock::Clock;
use crate::read::read;
use crate::views;

const LOOK_AGAIN: Duration = Duration::from_secs(3600);

pub(crate) async fn run(
    kick: Arc<Kick>,
    pool: Arc<ReadPool>,
    platform: Arc<dyn Platform>,
    clock: Arc<dyn Clock>,
) {
    loop {
        kick.wait(&*clock, LOOK_AGAIN).await;
        update_badge(&pool, &*platform).await;
    }
}

async fn update_badge(pool: &Arc<ReadPool>, platform: &dyn Platform) {
    match read(pool, |r| views::badge_count(**r)).await {
        Ok(count) => platform.set_badge((count > 0).then_some(count)),
        Err(e) => tracing::warn!(error = %e, "couldn't count unread mail for the badge"),
    }
}
