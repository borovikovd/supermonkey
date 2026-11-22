use std::sync::Arc;
use std::time::Duration;

use sm_domain::Millis;
use sm_store::{Priority, ReadPool};

use crate::activity::Kick;
use crate::clock::Clock;
use crate::writer::CoreWriter;

const LOOK_AGAIN: Duration = Duration::from_secs(60);

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

async fn next_snooze(pool: &Arc<ReadPool>) -> Option<Millis> {
    crate::read::read(pool, |r| r.threads().next_snooze())
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "couldn't read the next snooze");
            None
        })
}

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
