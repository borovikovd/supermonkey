use sm_domain::{Addr, MessageFacts, ThreadId, email_domain};
use sm_gmail::{Filter, FilterId, GmailError, HttpClient, Priority};
use sm_ipc::{BlockResult, BlockedSender, BlockedSenders, Unsubscribed};
use sm_mime::{Mailto, PlainMessage, build_plain, list_unsubscribe};

use super::account::AccountCtx;
use crate::read::read;
use crate::{Error, Platform};
use sm_domain::AccountId;
use sm_store::ReadPool;
use std::sync::Arc;

const HEADERS: [&str; 2] = ["List-Unsubscribe", "List-Unsubscribe-Post"];
const UNSUBSCRIBE: &str = "Unsubscribe";

pub(crate) async fn unsubscribe(
    ctx: &AccountCtx,
    http: &HttpClient,
    platform: &dyn Platform,
    thread: ThreadId,
) -> Result<Unsubscribed, Error> {
    let messages = thread_messages(&ctx.deps.pool, ctx.account, thread).await?;
    let Some(message) = unsubscribe_source(&messages) else {
        return Err(Error::Invalid(
            "The conversation has no message to unsubscribe from".to_owned(),
        ));
    };
    let sender = message.sender_display().to_owned();
    let read = ctx
        .gmail
        .get_message_headers(Priority::P1, &message.gmail_id, &HEADERS)
        .await;
    ctx.note(&read);
    let headers = read?;
    let Some(header) = headers.header(HEADERS[0]) else {
        return Ok(Unsubscribed::NoLink { sender });
    };
    let how = list_unsubscribe(header, headers.header(HEADERS[1]));
    if how.is_one_click
        && let Some(url) = &how.web
        && one_click(ctx, http, url).await
    {
        return Ok(Unsubscribed::OneClick { sender });
    }
    if let Some(mailto) = how.mailto {
        email_unsubscribe(ctx, &mailto).await?;
        return Ok(Unsubscribed::Emailed { sender });
    }
    if let Some(url) = how.web {
        platform.open_url(&url)?;
        return Ok(Unsubscribed::OpenedPage { sender });
    }
    Ok(Unsubscribed::NoLink { sender })
}

async fn one_click(ctx: &AccountCtx, http: &HttpClient, url: &str) -> bool {
    sm_gmail::one_click_unsubscribe(http, url)
        .await
        .inspect_err(|e| {
            tracing::warn!(account = ctx.account.0, outcome = %e.outcome(), "the one-click unsubscribe failed");
        })
        .is_ok()
}

async fn email_unsubscribe(ctx: &AccountCtx, mailto: &Mailto) -> Result<(), Error> {
    let raw = build_plain(&PlainMessage {
        from: &Addr::new(account_email(&ctx.deps.pool, ctx.account).await?),
        to: &mailto.to,
        subject: mailto.subject.as_deref().unwrap_or(UNSUBSCRIBE),
        text: mailto.body.as_deref().unwrap_or(UNSUBSCRIBE),
        date: ctx.clock().now(),
    });
    let sent = ctx.gmail.send_message(Priority::P1, &raw).await;
    ctx.note(&sent);
    sent?;
    Ok(())
}

fn unsubscribe_source(messages: &[MessageFacts]) -> Option<&MessageFacts> {
    let sent = || messages.iter().rev().filter(|m| !m.is_draft);
    sent().find(|m| m.is_list).or_else(|| sent().next())
}

pub(crate) async fn block(
    ctx: &AccountCtx,
    thread: ThreadId,
    domain: bool,
) -> Result<BlockResult, Error> {
    let messages = thread_messages(&ctx.deps.pool, ctx.account, thread).await?;
    let from = block_target(&messages, domain)?;
    let made = ctx.gmail.create_trash_filter(Priority::P1, &from).await;
    ctx.note(&made);
    filter_made(ctx.account, domain, from, made)
}

fn filter_made(
    account: AccountId,
    domain: bool,
    from: String,
    made: Result<Filter, GmailError>,
) -> Result<BlockResult, Error> {
    match made {
        Ok(_) => {
            tracing::info!(account = account.0, domain, "blocked a sender");
            Ok(BlockResult::Blocked { from })
        }
        Err(e) if e.outcome().is_missing_scope() => Ok(BlockResult::NeedsPermission),
        Err(e) => Err(e.into()),
    }
}

fn block_target(messages: &[MessageFacts], domain: bool) -> Result<String, Error> {
    let Some(email) = messages
        .iter()
        .rev()
        .find(|m| !m.is_draft && !m.from.email.is_empty())
        .map(|m| m.from.email.to_ascii_lowercase())
    else {
        return Err(Error::Invalid(
            "The conversation has no sender to block".to_owned(),
        ));
    };
    if !domain {
        return Ok(email);
    }
    let Some(at) = email_domain(&email) else {
        return Err(Error::Invalid("The sender has no domain".to_owned()));
    };
    Ok(format!("@{at}"))
}

pub(crate) async fn blocked_senders(ctx: &AccountCtx) -> Result<BlockedSenders, Error> {
    let listed = ctx.gmail.list_filters(Priority::P1).await;
    ctx.note(&listed);
    match listed {
        Ok(filters) => Ok(BlockedSenders::List {
            senders: filters.into_iter().filter_map(blocked_sender).collect(),
        }),
        Err(e) if e.outcome().is_missing_scope() => Ok(BlockedSenders::NeedsPermission),
        Err(e) => Err(e.into()),
    }
}

pub(crate) async fn unblock(ctx: &AccountCtx, filter: &FilterId) -> Result<(), Error> {
    let listed = ctx.gmail.list_filters(Priority::P1).await;
    ctx.note(&listed);
    let is_block = listed?
        .into_iter()
        .filter(|f| f.id == *filter)
        .find_map(blocked_sender)
        .is_some();
    if !is_block {
        return Err(Error::Invalid("That sender is not blocked".to_owned()));
    }
    let deleted: Result<(), GmailError> = ctx.gmail.delete_filter(Priority::P1, filter).await;
    ctx.note(&deleted);
    deleted?;
    tracing::info!(account = ctx.account.0, "unblocked a sender");
    Ok(())
}

fn blocked_sender(filter: Filter) -> Option<BlockedSender> {
    let Filter {
        id,
        criteria,
        action,
    } = filter;
    let from = criteria.from?;
    let trashes = action.add_label_ids.iter().any(|l| l.as_str() == "TRASH");
    (criteria.other.is_empty() && trashes).then_some(BlockedSender { id: id.0, from })
}

async fn thread_messages(
    pool: &Arc<ReadPool>,
    account: AccountId,
    thread: ThreadId,
) -> Result<Vec<MessageFacts>, Error> {
    let (owner, messages) = read(pool, move |r| {
        Ok((r.threads().account_of(thread)?, r.messages().facts(thread)?))
    })
    .await?;
    if owner != Some(account) {
        return Err(Error::Invalid(
            "The conversation is not in this account".to_owned(),
        ));
    }
    Ok(messages)
}

async fn account_email(pool: &Arc<ReadPool>, account: AccountId) -> Result<String, Error> {
    read(pool, move |r| {
        Ok(r.accounts().get(account)?.map(|a| a.email))
    })
    .await?
    .ok_or_else(|| Error::Invalid("The account is gone".to_owned()))
}
