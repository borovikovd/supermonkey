use std::sync::Arc;
use std::time::Duration;

use sm_gmail::{Backoff, GmailError, Outcome};
use sm_store::{OpQueue, OpRecord};
use tokio::sync::{OwnedMutexGuard, oneshot};

use super::run::{Failure, run_op};

use crate::Error;
use crate::activity::parked;
use crate::workers::account::{AccountCtx, OFFLINE_RETRY};

const IDLE_WAIT: Duration = Duration::from_secs(60);
const BUSY_WAIT: Duration = Duration::from_secs(1);

pub(crate) async fn run(ctx: Arc<AccountCtx>, queue: OpQueue) {
    let mut backoff = Backoff::new();
    loop {
        let turn = ctx.lane.turn(queue).await;
        let Some((op, turn)) = next_op(&ctx, queue, turn).await else {
            continue;
        };
        let Some(result) = run_in_task(&ctx, op, turn).await else {
            return;
        };
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

async fn next_op(
    ctx: &AccountCtx,
    queue: OpQueue,
    turn: OwnedMutexGuard<()>,
) -> Option<(OpRecord, OwnedMutexGuard<()>)> {
    let account = ctx.account;
    match ctx.read(move |r| r.ops().next(account, queue)).await {
        Ok(Some(op)) => Some((op, turn)),
        Ok(None) => {
            drop(turn);
            ctx.ops(queue).wait(ctx.clock(), IDLE_WAIT).await;
            None
        }
        Err(e) => {
            drop(turn);
            wait_after_read_error(ctx, queue, &e).await;
            None
        }
    }
}

async fn wait_after_read_error(ctx: &AccountCtx, queue: OpQueue, e: &Error) {
    tracing::warn!(account = ctx.account.0, error = %e, "couldn't read the op queue");
    ctx.ops(queue).wait(ctx.clock(), OFFLINE_RETRY).await;
}

async fn run_in_task(
    ctx: &Arc<AccountCtx>,
    op: OpRecord,
    turn: OwnedMutexGuard<()>,
) -> Option<Result<(), Failure>> {
    let (done, ran) = oneshot::channel();
    let runner = Arc::clone(ctx);
    ctx.lane.tasks().spawn("op", async move {
        let result = run_op(&runner, op).await;
        drop(turn);
        _ = done.send(runner.tasks.activity().track("op result", result));
    });
    let Ok(ran) = parked(ran).await else {
        tracing::debug!(
            account = ctx.account.0,
            "an op run was cancelled; the account is going"
        );
        return None;
    };
    let (result, _) = ran.into_parts();
    Some(result)
}

async fn wait_after(
    ctx: &AccountCtx,
    queue: OpQueue,
    failure: &Failure,
    backoff: &mut Backoff,
) -> bool {
    let Some(wait) = retry_wait(ctx, failure, backoff) else {
        return false;
    };
    draining(ctx, queue, ctx.deps.network.wait(ctx.clock(), wait)).await;
    true
}

fn retry_wait(ctx: &AccountCtx, failure: &Failure, backoff: &mut Backoff) -> Option<Duration> {
    match failure {
        Failure::Store(e) => {
            tracing::warn!(account = ctx.account.0, error = %e, "couldn't run an op; trying again");
            Some(OFFLINE_RETRY)
        }
        Failure::Busy => Some(BUSY_WAIT),
        Failure::Gmail(err) => gmail_wait(ctx, err, backoff),
    }
}

fn gmail_wait(ctx: &AccountCtx, err: &GmailError, backoff: &mut Backoff) -> Option<Duration> {
    ctx.health.failed(err);
    match err.outcome() {
        Outcome::InvalidGrant => {
            ctx.signed_out();
            None
        }
        Outcome::Offline => Some(OFFLINE_RETRY),
        outcome @ (Outcome::NoResponse
        | Outcome::Unauthorized
        | Outcome::RateLimited { .. }
        | Outcome::ServerError { .. }
        | Outcome::NotFound
        | Outcome::Conflict
        | Outcome::Rejected { .. }) => {
            let wait = backoff.next_delay(err);
            tracing::debug!(account = ctx.account.0, %outcome, ?wait, "an op backs off");
            Some(wait)
        }
    }
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
