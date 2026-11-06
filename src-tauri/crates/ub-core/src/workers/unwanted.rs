//! Unsubscribe and block (SPEC Unsubscribe and block). Both need the network
//! and are not queued: a queued unsubscribe could fire long after the user
//! stopped caring.

use ub_domain::{Addr, MessageFacts, ThreadId, email_domain};
use ub_gmail::{Filter, FilterId, GmailError, HttpClient, Priority};
use ub_ipc::{BlockResult, BlockedSender, BlockedSenders, Unsubscribed};
use ub_mime::{PlainMessage, build_plain, list_unsubscribe};

use super::account::AccountCtx;
use crate::read::read;
use crate::{Error, Platform};
use std::sync::Arc;
use ub_domain::AccountId;
use ub_store::ReadPool;

/// The headers an unsubscribe reads.
const HEADERS: [&str; 2] = ["List-Unsubscribe", "List-Unsubscribe-Post"];
/// What an unsubscribe email says when the list names nothing.
const UNSUBSCRIBE: &str = "Unsubscribe";

/// Unsubscribes from the list `thread`'s latest list message came from (or
/// its latest message): reads its unsubscribe headers from Gmail, then
/// posts the one-click request, emails the list, or opens its page, in that
/// order.
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
    {
        match ub_gmail::one_click_unsubscribe(http, url).await {
            Ok(()) => return Ok(Unsubscribed::OneClick { sender }),
            // The list's other ways are still there.
            Err(e) => {
                tracing::warn!(account = ctx.account.0, outcome = %e.outcome(), "the one-click unsubscribe failed");
            }
        }
    }
    if let Some(mailto) = how.mailto {
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
        return Ok(Unsubscribed::Emailed { sender });
    }
    if let Some(url) = how.web {
        platform.open_url(&url)?;
        return Ok(Unsubscribed::OpenedPage { sender });
    }
    Ok(Unsubscribed::NoLink { sender })
}

/// The message an unsubscribe reads: the latest list message, else the
/// latest message, drafts aside. Trashed ones count: "Unsubscribe, and trash
/// all" trashes the conversation first, and that may commit before this
/// reads it.
fn unsubscribe_source(messages: &[MessageFacts]) -> Option<&MessageFacts> {
    let sent = || messages.iter().rev().filter(|m| !m.is_draft);
    sent().find(|m| m.is_list).or_else(|| sent().next())
}

/// Blocks the sender of `thread`'s latest message, or everyone at its
/// domain, with a Gmail filter that sends their mail to Trash. An account
/// whose grant lacks the filters scope needs to sign in again.
pub(crate) async fn block(
    ctx: &AccountCtx,
    thread: ThreadId,
    domain: bool,
) -> Result<BlockResult, Error> {
    let messages = thread_messages(&ctx.deps.pool, ctx.account, thread).await?;
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
    let from = if domain {
        let Some(at) = email_domain(&email) else {
            return Err(Error::Invalid("The sender has no domain".to_owned()));
        };
        format!("@{at}")
    } else {
        email
    };
    let made = ctx.gmail.create_trash_filter(Priority::P1, &from).await;
    ctx.note(&made);
    match made {
        Ok(_) => {
            tracing::info!(account = ctx.account.0, domain, "blocked a sender");
            Ok(BlockResult::Blocked { from })
        }
        Err(e) if e.outcome().is_missing_scope() => Ok(BlockResult::NeedsPermission),
        Err(e) => Err(e.into()),
    }
}

/// The account's blocked senders: its filters that match only a sender and
/// send it to Trash, the shape [`block`] makes. Filters the user made in
/// Gmail for other things are left out.
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

/// Unblocks a sender by deleting its filter. The id comes from the webview,
/// which is not trusted: only one of the account's block filters (as
/// Blocked senders lists them) is deleted, never another filter.
pub(crate) async fn unblock(ctx: &AccountCtx, filter: FilterId) -> Result<(), Error> {
    let listed = ctx.gmail.list_filters(Priority::P1).await;
    ctx.note(&listed);
    let is_block = listed?
        .into_iter()
        .filter(|f| f.id == filter)
        .find_map(blocked_sender)
        .is_some();
    if !is_block {
        return Err(Error::Invalid("That sender is not blocked".to_owned()));
    }
    let deleted: Result<(), GmailError> = ctx.gmail.delete_filter(Priority::P1, &filter).await;
    ctx.note(&deleted);
    deleted?;
    tracing::info!(account = ctx.account.0, "unblocked a sender");
    Ok(())
}

/// A filter that blocks a sender: it matches only `from` and adds `TRASH`.
fn blocked_sender(filter: Filter) -> Option<BlockedSender> {
    let Filter {
        id,
        criteria,
        action,
    } = filter;
    let from = criteria.from?;
    let trashes = action.add_label_ids.iter().any(|l| l.as_str() == "TRASH");
    (criteria.other.is_empty() && trashes).then(|| BlockedSender {
        id: id.as_str().to_owned(),
        from,
    })
}

/// The thread's messages, oldest first; the thread must be the account's,
/// since the webview is not trusted.
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

/// The account's address, which an unsubscribe email comes from.
async fn account_email(pool: &Arc<ReadPool>, account: AccountId) -> Result<String, Error> {
    read(pool, move |r| {
        Ok(r.accounts().get(account)?.map(|a| a.email))
    })
    .await?
    .ok_or_else(|| Error::Invalid("The account is gone".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(json: &str) -> Filter {
        serde_json::from_str(json).unwrap()
    }

    fn fact(id: i64, is_list: bool, is_draft: bool, labels: &[&str]) -> MessageFacts {
        MessageFacts {
            id: ub_domain::MessageId(id),
            gmail_id: ub_domain::GmailMessageId::from(format!("m{id}")),
            date: ub_domain::Millis(id),
            from: Addr::new("news@news.test"),
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            snippet: String::new(),
            is_draft,
            is_list,
            has_attachments: false,
            labels: labels.iter().copied().collect(),
        }
    }

    #[test]
    fn unsubscribe_reads_the_latest_list_message_even_once_trashed() {
        let trashed = [
            fact(1, true, false, &["TRASH"]),
            fact(2, true, false, &["TRASH"]),
            fact(3, false, false, &["TRASH"]),
            fact(4, false, true, &["DRAFT"]),
        ];
        assert_eq!(unsubscribe_source(&trashed).map(|m| m.id.0), Some(2));
        let plain = [
            fact(1, false, false, &["INBOX"]),
            fact(2, false, true, &["DRAFT"]),
        ];
        assert_eq!(unsubscribe_source(&plain).map(|m| m.id.0), Some(1));
        assert!(unsubscribe_source(&[fact(1, false, true, &["DRAFT"])]).is_none());
    }

    #[test]
    fn only_filters_that_trash_one_sender_are_blocks() {
        let block = filter(
            r#"{"id":"f1","criteria":{"from":"news@news.test"},"action":{"addLabelIds":["TRASH"],"removeLabelIds":["INBOX"]}}"#,
        );
        let narrower = filter(
            r#"{"id":"f2","criteria":{"from":"boss@corp.test","subject":"urgent"},"action":{"addLabelIds":["TRASH"]}}"#,
        );
        let archives = filter(
            r#"{"id":"f3","criteria":{"from":"list@corp.test"},"action":{"removeLabelIds":["INBOX"]}}"#,
        );
        let no_sender = filter(
            r#"{"id":"f4","criteria":{"query":"unsubscribe"},"action":{"addLabelIds":["TRASH"]}}"#,
        );
        let blocks: Vec<BlockedSender> = [block, narrower, archives, no_sender]
            .into_iter()
            .filter_map(blocked_sender)
            .collect();
        assert_eq!(
            blocks,
            [BlockedSender {
                id: "f1".to_owned(),
                from: "news@news.test".to_owned(),
            }]
        );
    }
}
