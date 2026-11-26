use crate::domain::{AccountId, Addr, ThreadId, email_domain};
use crate::gmail::{HttpClient, Priority as GmailPriority, one_click_unsubscribe};
use crate::ipc::{BlockResult, BlockedSender, BlockedSenders, Unsubscribed};
use crate::mime::{Mailto, PlainMessage, build_plain, list_unsubscribe};
use crate::store::{ReadPool, StoreError};

use super::runtime::Clock;
use super::sync::{AccountGmail, AccountManager};
use super::{CoreError, Platform};

async fn send_unsubscribe_mailto(
    client: &AccountGmail,
    acct_rec: &crate::store::AccountRecord,
    mailto: Mailto,
    clock: &Clock,
) -> Result<(), CoreError> {
    let from = acct_rec.identity().default_from();
    let to = Addr::new(mailto.to);
    let subject = if mailto.subject.is_empty() {
        "Unsubscribe".to_owned()
    } else {
        mailto.subject
    };
    let raw = build_plain(&PlainMessage {
        from: &from,
        to: &to,
        subject: &subject,
        body: &mailto.body,
        date: clock.now(),
    })
    .map_err(|e| CoreError::internal(e.to_string()))?;
    client
        .check(
            client
                .gmail()
                .send_message(&raw, None, GmailPriority::P1)
                .await,
        )
        .await
        .map_err(|e| CoreError::other(format!("Couldn't unsubscribe: {e}")))?;
    Ok(())
}

fn newest_from_someone_else(
    msgs: &[crate::store::MessageRecord],
    identity: &crate::domain::Identity,
) -> Option<usize> {
    msgs.iter()
        .rposition(|m| m.is_live && !identity.is_mine(&m.from.email))
}

fn select_unsubscribe_target(
    pool: &ReadPool,
    account: AccountId,
    thread: ThreadId,
) -> Result<(crate::store::AccountRecord, crate::store::MessageRecord), CoreError> {
    pool.with(|db| {
        let acct = db
            .account(account)?
            .ok_or(StoreError::NotFound("account"))?;
        let row = db
            .thread_row(thread)?
            .ok_or(StoreError::NotFound("thread"))?;
        if row.account != account {
            return Err(StoreError::Invalid("thread belongs to another account"));
        }
        let mut msgs = db.thread_messages(thread)?;
        let identity = acct.identity();
        let pos = newest_from_someone_else(&msgs, &identity)
            .or_else(|| msgs.iter().rposition(|m| !m.is_draft))
            .ok_or(StoreError::NotFound("message"))?;
        Ok::<_, StoreError>((acct, msgs.swap_remove(pos)))
    })
    .map_err(|e| CoreError::other(e.to_string()))
}

#[expect(
    clippy::too_many_arguments,
    reason = "unsubscribe resolves thread message headers, dispatches HTTP/mailto/browser action, and reports result"
)]
pub(crate) async fn unsubscribe_thread(
    account: AccountId,
    thread: ThreadId,
    accounts: &AccountManager,
    pool: &ReadPool,
    http: &HttpClient,
    clock: &Clock,
    platform: &dyn Platform,
) -> Result<Unsubscribed, CoreError> {
    let (acct_rec, target_msg) = select_unsubscribe_target(pool, account, thread)?;

    let sender = target_msg.from.display_name().to_owned();
    let client = accounts.gmail_session(account, "Sign in again to unsubscribe")?;
    client.ensure_online("Couldn't unsubscribe while offline")?;

    let meta = client
        .check(
            client
                .gmail()
                .get_message_headers(
                    &target_msg.gmail_id,
                    &["List-Unsubscribe", "List-Unsubscribe-Post"],
                    GmailPriority::P1,
                )
                .await,
        )
        .await
        .map_err(|err| CoreError::other(format!("Couldn't unsubscribe: {err}")))?;

    let parsed = list_unsubscribe(
        meta.header("List-Unsubscribe"),
        meta.header("List-Unsubscribe-Post"),
    );

    if let Some(url) = parsed.one_click {
        one_click_unsubscribe(http, &url)
            .await
            .map_err(|e| CoreError::other(format!("Couldn't unsubscribe: {e}")))?;
        return Ok(Unsubscribed::OneClick { sender });
    }
    if let Some(mailto) = parsed.mailto {
        send_unsubscribe_mailto(&client, &acct_rec, mailto, clock).await?;
        return Ok(Unsubscribed::Emailed { sender });
    }
    if let Some(url) = parsed.url {
        platform.open_url(&url)?;
        return Ok(Unsubscribed::OpenedPage { sender });
    }
    Ok(Unsubscribed::NoLink { sender })
}

pub(crate) fn sender_threads(
    account: AccountId,
    thread: ThreadId,
    pool: &ReadPool,
) -> Result<Vec<ThreadId>, CoreError> {
    pool.with(|db| {
        let Some(row) = db.thread_row(thread)? else {
            return Ok(Vec::new());
        };
        if row.account != account {
            return Ok(Vec::new());
        }
        let Some(acct) = db.account(account)? else {
            return Ok(Vec::new());
        };
        let msgs = db.thread_messages(thread)?;
        let Some(pos) = newest_from_someone_else(&msgs, &acct.identity()) else {
            return Ok(Vec::new());
        };
        let Some(sender) = msgs.get(pos) else {
            return Ok(Vec::new());
        };
        db.sender_inbox_threads(account, &sender.from.email)
    })
    .map_err(|e| CoreError::internal(e.to_string()))
}

pub(crate) async fn block_sender(
    account: AccountId,
    thread: ThreadId,
    domain_wide: bool,
    accounts: &AccountManager,
    pool: &ReadPool,
) -> Result<BlockResult, CoreError> {
    let from_email = pool
        .with(|db| {
            let acct = db
                .account(account)?
                .ok_or(StoreError::NotFound("account"))?;
            let row = db
                .thread_row(thread)?
                .ok_or(StoreError::NotFound("thread"))?;
            if row.account != account {
                return Err(StoreError::Invalid("thread belongs to another account"));
            }
            let identity = acct.identity();
            let mut msgs = db.thread_messages(thread)?;
            let sender = newest_from_someone_else(&msgs, &identity)
                .map_or(row.sender_email, |pos| msgs.swap_remove(pos).from.email);
            Ok::<_, StoreError>(sender)
        })
        .map_err(|e| CoreError::other(e.to_string()))?;

    let pattern = if domain_wide {
        let dom = email_domain(&from_email)
            .ok_or_else(|| CoreError::other("Sender has no domain to block"))?;
        format!("@{dom}")
    } else {
        from_email.trim().to_ascii_lowercase()
    };

    let client = accounts.gmail_session(account, "Sign in again to block senders")?;
    client.ensure_online("Couldn't block sender while offline")?;

    match client
        .check(
            client
                .gmail()
                .create_trash_filter(&pattern, GmailPriority::P1)
                .await,
        )
        .await
    {
        Ok(_) => Ok(BlockResult::Blocked { from: pattern }),
        Err(err) if err.is_duplicate_filter() => Ok(BlockResult::Blocked { from: pattern }),
        Err(err) if err.is_missing_scope() => Ok(BlockResult::NeedsPermission),
        Err(err) => Err(CoreError::other(err.to_string())),
    }
}

pub(crate) async fn list_blocked_senders(
    account: AccountId,
    accounts: &AccountManager,
) -> Result<BlockedSenders, CoreError> {
    let client = accounts.gmail_session(account, "Sign in again to view blocked senders")?;
    match client
        .check(client.gmail().list_filters(GmailPriority::P1).await)
        .await
    {
        Ok(filters) => {
            let senders = filters
                .iter()
                .filter_map(|f| {
                    f.blocked_sender().map(|from| BlockedSender {
                        id: f.id.0.clone(),
                        from: from.to_owned(),
                    })
                })
                .collect();
            Ok(BlockedSenders::List { senders })
        }
        Err(err) if err.is_missing_scope() => Ok(BlockedSenders::NeedsPermission),
        Err(err) => Err(CoreError::other(err.to_string())),
    }
}

pub(crate) async fn unblock_sender(
    account: AccountId,
    filter_id: &str,
    accounts: &AccountManager,
) -> Result<(), CoreError> {
    let client = accounts.gmail_session(account, "Sign in again to unblock senders")?;
    let filters = client
        .check(client.gmail().list_filters(GmailPriority::P1).await)
        .await
        .map_err(|err| CoreError::other(err.to_string()))?;
    let Some(matched) = filters
        .into_iter()
        .find(|f| f.id.0 == filter_id && f.blocked_sender().is_some())
    else {
        return Err(CoreError::other("Blocked sender not found"));
    };
    client
        .check(
            client
                .gmail()
                .delete_filter(&matched.id, GmailPriority::P1)
                .await,
        )
        .await
        .map_err(|err| CoreError::other(err.to_string()))
}
