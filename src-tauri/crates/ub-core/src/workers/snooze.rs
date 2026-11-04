//! The snooze timer (SPEC Snooze): wakes each snoozed thread at its time on
//! the core's clock. Launch and wake from sleep wake the due ones too.

use std::sync::Arc;
use std::time::Duration;

use ub_domain::Millis;
use ub_store::{Priority, ReadPool};

use crate::activity::Kick;
use crate::clock::Clock;
use crate::writer::CoreWriter;

/// The longest the timer waits before it looks again. A snooze time set
/// without a kick, such as one a rejected op's rollback restores, still
/// wakes within this of its time.
const LOOK_AGAIN: Duration = Duration::from_secs(60);

/// Runs the timer until cancelled: waits for the soonest snooze, or a kick
/// when a snooze time is set, then wakes what is due.
pub(crate) async fn run(
    kick: Arc<Kick>,
    pool: Arc<ReadPool>,
    writer: CoreWriter,
    clock: Arc<dyn Clock>,
) {
    loop {
        let next = next_snooze(&pool).await;
        let now = clock.now();
        let wait = match next {
            Some(at) if at <= now => {
                if wake_due(&writer, now).await {
                    continue;
                }
                LOOK_AGAIN
            }
            Some(at) => {
                let ms = u64::try_from(at.0.saturating_sub(now.0)).unwrap_or(0);
                Duration::from_millis(ms).min(LOOK_AGAIN)
            }
            None => LOOK_AGAIN,
        };
        kick.wait(&*clock, wait).await;
    }
}

/// The soonest snooze end; `None` when there is none or it can't be read,
/// which is logged.
async fn next_snooze(pool: &Arc<ReadPool>) -> Option<Millis> {
    let pool = Arc::clone(pool);
    match tokio::task::spawn_blocking(move || pool.with(|r| r.threads().next_snooze())).await {
        Ok(Ok(next)) => next,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "couldn't read the next snooze");
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "the next-snooze read failed");
            None
        }
    }
}

/// Wakes the snoozes due by `now` and waits for the commit. Returns whether
/// it committed; a failure is logged.
pub(crate) async fn wake_due(writer: &CoreWriter, now: Millis) -> bool {
    let woke = writer
        .call(Priority::Sync, move |tx, state| {
            crate::snooze::wake_due(tx, state, now)
        })
        .await;
    match woke {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(error = %e, "couldn't wake due snoozes");
            false
        }
    }
}
