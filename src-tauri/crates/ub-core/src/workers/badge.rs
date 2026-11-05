//! The Dock badge (SPEC Notifications): unread Inbox threads across the
//! accounts not muted, recounted after commits that may change it. Its own
//! task, woken by a kick, so a burst of commits (the connect sync stores
//! one per thread) costs one recount per wake, not one per commit, and the
//! effect router never waits on a count.

use std::sync::Arc;
use std::time::Duration;

use ub_store::ReadPool;

use crate::Platform;
use crate::activity::Kick;
use crate::clock::Clock;
use crate::read::read;
use crate::views;

/// How long the task waits for a kick before it counts again anyway.
const LOOK_AGAIN: Duration = Duration::from_secs(3600);

/// Recounts the badge on each kick until cancelled. Kicks that come while a
/// count runs make one more count after it.
pub(crate) async fn run(
    kick: Arc<Kick>,
    pool: Arc<ReadPool>,
    platform: Arc<dyn Platform>,
    clock: Arc<dyn Clock>,
) {
    loop {
        kick.wait(&*clock, LOOK_AGAIN).await;
        match read(&pool, |r| views::badge_count(**r)).await {
            Ok(count) => platform.set_badge((count > 0).then_some(count)),
            Err(e) => tracing::warn!(error = %e, "couldn't count unread mail for the badge"),
        }
    }
}
