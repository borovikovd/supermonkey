//! The ops runner's loop: takes the account's next op in the lane's turn,
//! runs it in the lane's task group, and waits as its outcome asks.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;
use ub_gmail::{Backoff, Outcome};

use super::run::{Failure, run_op};

use crate::activity::parked;
use crate::workers::account::{AccountCtx, OFFLINE_RETRY};

/// How long the runner waits for a kick before it looks at the queue
/// again anyway.
const IDLE_WAIT: Duration = Duration::from_secs(60);
/// How long an op whose draft the outbox is writing waits before it looks
/// again.
const BUSY_WAIT: Duration = Duration::from_secs(1);

/// Replays the account's ops until the worker stops.
pub(crate) async fn run(ctx: Arc<AccountCtx>) {
    let mut backoff = Backoff::new();
    loop {
        let turn = ctx.lane.turn().await;
        let account = ctx.account;
        let op = match ctx.read(move |r| r.ops().next(account)).await {
            Ok(Some(op)) => op,
            Ok(None) => {
                drop(turn);
                ctx.ops.wait(ctx.clock(), IDLE_WAIT).await;
                continue;
            }
            Err(e) => {
                drop(turn);
                tracing::warn!(account = account.0, error = %e, "couldn't read the op queue");
                ctx.ops.wait(ctx.clock(), OFFLINE_RETRY).await;
                continue;
            }
        };
        let (done, ran) = oneshot::channel();
        let runner = Arc::clone(&ctx);
        ctx.lane.tasks().spawn("op", async move {
            let result = run_op(&runner, op).await;
            drop(turn);
            // Busy until the runner has taken it, so the core never looks
            // idle between this run and the next. The worker that asked may
            // have been replaced meanwhile.
            let _ = done.send(runner.tasks.activity().track("op result", result));
        });
        // Parked: the run counts as busy, and its result carries its own
        // activity. A held Gmail request then counts once (`settle`).
        let Ok(ran) = parked(ran).await else {
            tracing::debug!(
                account = account.0,
                "an op run was cancelled; the account is going"
            );
            return;
        };
        // Resumed: this task counts again.
        let (result, _) = ran.into_parts();
        match result {
            Ok(()) => backoff.reset(),
            Err(failure) => {
                if !wait_after(&ctx, &failure, &mut backoff).await {
                    return;
                }
            }
        }
    }
}

/// Waits as a failed run's outcome asks (SPEC Gmail errors). Returns false
/// when the runner should stop: the account is signed out.
async fn wait_after(ctx: &AccountCtx, failure: &Failure, backoff: &mut Backoff) -> bool {
    let network = &ctx.deps.network;
    match failure {
        Failure::Store(e) => {
            tracing::warn!(account = ctx.account.0, error = %e, "couldn't run an op; trying again");
            draining(ctx, network.wait(ctx.clock(), OFFLINE_RETRY)).await;
        }
        Failure::Busy => {
            draining(ctx, network.wait(ctx.clock(), BUSY_WAIT)).await;
        }
        Failure::Gmail(err) => {
            ctx.health.failed(err);
            match err.outcome() {
                Outcome::InvalidGrant => {
                    ctx.signed_out();
                    return false;
                }
                Outcome::Offline => draining(ctx, network.wait(ctx.clock(), OFFLINE_RETRY)).await,
                outcome => {
                    let wait = backoff.next_delay(err);
                    tracing::debug!(account = ctx.account.0, %outcome, ?wait, "an op backs off");
                    draining(ctx, network.wait(ctx.clock(), wait)).await;
                }
            }
        }
    }
    true
}

/// Waits out `wait`, taking every kick that comes meanwhile: the runner
/// goes back to the queue when the wait ends, so a kick asks for nothing
/// more, and a kick nobody took would keep the core busy. A kick never
/// cuts a wait short, so a back-off keeps its timing.
async fn draining(ctx: &AccountCtx, wait: impl Future<Output = ()>) {
    tokio::pin!(wait);
    loop {
        // The wait first, so its timer is set before this task parks.
        tokio::select! {
            biased;
            () = &mut wait => return,
            () = ctx.ops.wait(ctx.clock(), IDLE_WAIT) => {}
        }
    }
}
