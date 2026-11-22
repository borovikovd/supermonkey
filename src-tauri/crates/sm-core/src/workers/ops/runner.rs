use std::sync::Arc;
use std::time::Duration;

use sm_gmail::{Backoff, Outcome};
use sm_store::OpQueue;
use tokio::sync::oneshot;

use super::run::{Failure, run_op};

use crate::activity::parked;
use crate::workers::account::{AccountCtx, OFFLINE_RETRY};

const IDLE_WAIT: Duration = Duration::from_secs(60);
const BUSY_WAIT: Duration = Duration::from_secs(1);

pub(crate) async fn run(ctx: Arc<AccountCtx>, queue: OpQueue) {
    let mut backoff = Backoff::new();
    loop {
        let turn = ctx.lane.turn(queue).await;
        let account = ctx.account;
        let op = match ctx.read(move |r| r.ops().next(account, queue)).await {
            Ok(Some(op)) => op,
            Ok(None) => {
                drop(turn);
                ctx.ops(queue).wait(ctx.clock(), IDLE_WAIT).await;
                continue;
            }
            Err(e) => {
                drop(turn);
                tracing::warn!(account = account.0, error = %e, "couldn't read the op queue");
                ctx.ops(queue).wait(ctx.clock(), OFFLINE_RETRY).await;
                continue;
            }
        };
        let (done, ran) = oneshot::channel();
        let runner = Arc::clone(&ctx);
        ctx.lane.tasks().spawn("op", async move {
            let result = run_op(&runner, op).await;
            drop(turn);
            _ = done.send(runner.tasks.activity().track("op result", result));
        });
        let Ok(ran) = parked(ran).await else {
            tracing::debug!(
                account = account.0,
                "an op run was cancelled; the account is going"
            );
            return;
        };
        let (result, _) = ran.into_parts();
        match result {
            Ok(()) => backoff.reset(),
            Err(failure) => {
                if !wait_after(&ctx, queue, &failure, &mut backoff).await {
                    return;
                }
            }
        }
    }
}

async fn wait_after(
    ctx: &AccountCtx,
    queue: OpQueue,
    failure: &Failure,
    backoff: &mut Backoff,
) -> bool {
    let network = &ctx.deps.network;
    match failure {
        Failure::Store(e) => {
            tracing::warn!(account = ctx.account.0, error = %e, "couldn't run an op; trying again");
            draining(ctx, queue, network.wait(ctx.clock(), OFFLINE_RETRY)).await;
        }
        Failure::Busy => {
            draining(ctx, queue, network.wait(ctx.clock(), BUSY_WAIT)).await;
        }
        Failure::Gmail(err) => {
            ctx.health.failed(err);
            match err.outcome() {
                Outcome::InvalidGrant => {
                    ctx.signed_out();
                    return false;
                }
                Outcome::Offline => {
                    draining(ctx, queue, network.wait(ctx.clock(), OFFLINE_RETRY)).await;
                }
                outcome @ (Outcome::NoResponse
                | Outcome::Unauthorized
                | Outcome::RateLimited { .. }
                | Outcome::ServerError { .. }
                | Outcome::NotFound
                | Outcome::Conflict
                | Outcome::Rejected { .. }) => {
                    let wait = backoff.next_delay(err);
                    tracing::debug!(account = ctx.account.0, %outcome, ?wait, "an op backs off");
                    draining(ctx, queue, network.wait(ctx.clock(), wait)).await;
                }
            }
        }
    }
    true
}

async fn draining(ctx: &AccountCtx, queue: OpQueue, wait: impl Future<Output = ()>) {
    tokio::pin!(wait);
    loop {
        tokio::select! {
            biased;
            () = &mut wait => return,
            () = ctx.ops(queue).wait(ctx.clock(), IDLE_WAIT) => {}
        }
    }
}
