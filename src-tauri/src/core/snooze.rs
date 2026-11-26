use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::domain::{OpKind, SystemLabel, ThreadChange};
use crate::store::{NewOp, Priority as StorePriority, ReadPool, StoreError, Writer};

use super::events::Effect;
use super::runtime::{Activity, Clock, Kick, WaitOutcome};
use super::writer_state::{ModifyOpPayload, WriterState};

pub(crate) async fn wake_due_snoozes(
    writer: &Writer<WriterState>,
    clock: &Clock,
) -> Result<usize, StoreError> {
    let now = clock.now();
    writer
        .call(StorePriority::Sync, move |tx, state| {
            let due = tx.read().due_snoozes(now)?;
            let mut woken = 0usize;
            for (account, tid) in due {
                let msgs = tx.read().thread_messages(tid)?;
                let mut gmail_msg_ids = Vec::new();
                for msg in &msgs {
                    if !msg.is_draft
                        && !msg.labels.has(SystemLabel::Trash)
                        && !msg.labels.has(SystemLabel::Spam)
                        && !msg.labels.has(SystemLabel::Inbox)
                    {
                        let next = msg.labels.and(&[SystemLabel::Inbox.label_id()]);
                        tx.set_message_labels(msg.id, &next)?;
                        gmail_msg_ids.push(msg.gmail_id.clone());
                    }
                }
                let before = tx.read().thread_row(tid)?.and_then(|r| r.snooze_until);
                tx.set_thread_snooze(tid, None)?;
                tx.derive_thread(tid)?;
                if !gmail_msg_ids.is_empty() {
                    let payload = ModifyOpPayload {
                        thread: tid,
                        gmail_msg_ids,
                        add: vec![SystemLabel::Inbox.label_id()],
                        remove: Vec::new(),
                        revert: ThreadChange {
                            thread: tid,
                            messages: Vec::new(),
                            snooze_before: before,
                            snooze_after: None,
                        },
                    };
                    tx.queue_op(&NewOp {
                        account,
                        action: None,
                        kind: OpKind::Modify,
                        draft_id: None,
                        gmail_draft_id: None,
                        payload: &payload,
                    })?;
                    state.emit(Effect::KickOps(account));
                }
                woken += 1;
            }
            if woken > 0 {
                state.emit(Effect::RefreshBadge);
            }
            Ok(woken)
        })
        .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "snooze loop coordinates read pool, writer, clock, kick, activity, and cancellation"
)]
pub(crate) async fn run_snooze_loop(
    pool: Arc<ReadPool>,
    writer: Writer<WriterState>,
    clock: Clock,
    kick: Arc<Kick>,
    activity: Activity,
    cancel: CancellationToken,
) {
    loop {
        if cancel.is_cancelled() {
            return;
        }
        #[expect(
            clippy::redundant_closure_for_method_calls,
            reason = "Db carries a lifetime so a closure is needed for ReadPool::with HRTB"
        )]
        let next = pool.with(|db| db.next_snooze()).ok().flatten();
        let guard = if let Some(at) = next {
            let now = clock.now();
            let ms = u64::try_from(at.0.saturating_sub(now.0)).unwrap_or(0);
            if ms == 0 {
                (Some(activity.enter()), None)
            } else {
                let (outcome, g) = kick
                    .wait_timeout(&clock, Duration::from_millis(ms), &activity, &cancel)
                    .await;
                if outcome == WaitOutcome::Cancelled {
                    return;
                }
                (None, g)
            }
        } else {
            let Some(g) = kick.wait(&activity, &cancel).await else {
                return;
            };
            (None, Some(g))
        };
        super::log_err("wake due snoozes", wake_due_snoozes(&writer, &clock).await);
        drop(guard);
    }
}
