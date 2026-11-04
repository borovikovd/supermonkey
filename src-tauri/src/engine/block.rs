//! Block (SPEC: Unsubscribe and block): Gmail filters that send a sender's or
//! a domain's mail to Trash, so Gmail itself keeps it out of the Inbox, with
//! the app closed and on every device.

use std::sync::Arc;

use anyhow::Context as _;
use ub_gmail::{Filter, Priority, cost};

use super::{AccountRt, Engine};
use crate::ids::{AccountId, ThreadId};
use crate::model::{BlockResult, BlockedSender, BlockedSenders};

/// Blocks the sender of `thread`'s latest message, or that sender's whole domain.
pub async fn block(
    e: &Arc<Engine>,
    account: AccountId,
    thread: ThreadId,
    domain: bool,
) -> anyhow::Result<BlockResult> {
    let (owner, msgs) = e.reads.with(|c| {
        Ok((
            crate::threads::account_of(c, thread)?,
            crate::threads::messages(c, thread)?,
        ))
    })?;
    anyhow::ensure!(
        owner == Some(account),
        "thread {thread} is not in account {account}"
    );
    let email = msgs
        .iter()
        .rev()
        .find(|m| !m.is_draft && !m.from_email.is_empty())
        .map(|m| m.from_email.to_ascii_lowercase())
        .context("the conversation has no sender to block")?;
    let from = if domain {
        let (_, d) = email.rsplit_once('@').context("the sender has no domain")?;
        format!("@{d}")
    } else {
        email
    };
    let rt = runtime(e, account)?;
    let res = rt
        .sched
        .run(
            Priority::P1,
            cost::FILTERS_CREATE,
            rt.gmail.create_trash_filter(&from),
        )
        .await;
    e.note(&rt, &res);
    match res {
        Ok(_) => Ok(BlockResult::Blocked { from }),
        Err(err) if err.outcome.is_missing_scope() => Ok(BlockResult::NeedsPermission),
        Err(err) => Err(anyhow::anyhow!("{}", err.outcome)),
    }
}

/// The account's Gmail filters that only send one sender or domain to Trash.
pub async fn blocked(e: &Arc<Engine>, account: AccountId) -> anyhow::Result<BlockedSenders> {
    let rt = runtime(e, account)?;
    let res = rt
        .sched
        .run(Priority::P1, cost::FILTERS_LIST, rt.gmail.list_filters())
        .await;
    e.note(&rt, &res);
    match res {
        Ok(filters) => Ok(BlockedSenders::List {
            senders: filters.iter().filter_map(blocked_sender).collect(),
        }),
        Err(err) if err.outcome.is_missing_scope() => Ok(BlockedSenders::NeedsPermission),
        Err(err) => Err(anyhow::anyhow!("{}", err.outcome)),
    }
}

/// Deletes the filter behind a blocked sender.
pub async fn unblock(e: &Arc<Engine>, account: AccountId, id: &str) -> anyhow::Result<()> {
    let rt = runtime(e, account)?;
    let res = rt
        .sched
        .run(
            Priority::P1,
            cost::FILTERS_DELETE,
            rt.gmail.delete_filter(id),
        )
        .await;
    e.note(&rt, &res);
    res.map_err(|err| anyhow::anyhow!("{}", err.outcome))
}

/// A filter is a block when it matches only a sender and sends it to Trash,
/// like the ones [`block`] makes; others the user made in Gmail are left out.
fn blocked_sender(f: &Filter) -> Option<BlockedSender> {
    let from = f.criteria.from.as_ref()?;
    (f.criteria.other.is_empty() && f.action.add_label_ids.iter().any(|l| l == "TRASH")).then(
        || BlockedSender {
            id: f.id.clone(),
            from: from.clone(),
        },
    )
}

fn runtime(e: &Engine, account: AccountId) -> anyhow::Result<Arc<AccountRt>> {
    e.account(account).context("the account is not signed in")
}
