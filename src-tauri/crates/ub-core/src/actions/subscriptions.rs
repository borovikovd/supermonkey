//! Opening and closing subscriptions (SPEC UI state, Subscriptions). These
//! are actions, so the writer sees them in order with the others; the first
//! load goes out with the commit.

use ub_domain::{AccountId, GmailListing, SubId};
use ub_ipc::SubKey;
use ub_store::{Reads, StoreError};

use super::{ActionCtx, ActionError};
use crate::effects::{Effect, GmailQuery};

/// A reloaded page's first action: closes every subscription of the
/// previous page. An open composer among them closes first, as a second
/// `Esc` would close it (SPEC UI state, Subscriptions).
pub(super) fn reset(ctx: &mut ActionCtx<'_, '_>) -> Result<(), ActionError> {
    for draft in ctx.state.subs.composer_drafts() {
        crate::drafts::close(ctx.tx, ctx.state, &draft, ctx.now)?;
    }
    for (id, key) in ctx.state.subs.close_all() {
        ctx.state.sub_effect(Effect::CancelSub(id));
        released(ctx, &key);
    }
    Ok(())
}

/// Opens a subscription, and asks for the work that showing it needs: the
/// thread's bodies, a draft's parts, a Gmail listing.
pub(super) fn subscribe(
    ctx: &mut ActionCtx<'_, '_>,
    sub: SubId,
    key: SubKey,
) -> Result<(), ActionError> {
    // Every read first, so a failed one records nothing.
    let reads = ctx.tx.read();
    let mut effects = Vec::new();
    match &key {
        SubKey::Conversation { thread, .. } => {
            // Requests of different accounts never wait on each other, so
            // the account on screen needs no head start (DEVIATIONS).
            if let Some(account) = reads.threads().account_of(*thread)? {
                effects.push(Effect::FetchThread {
                    account,
                    thread: *thread,
                });
            }
        }
        SubKey::Composer { draft } => {
            if let Some(record) = reads.drafts().get(draft)?
                && record.draft.needs_parts
            {
                effects.push(Effect::DraftParts {
                    account: record.draft.account,
                    draft: draft.clone(),
                });
            }
        }
        SubKey::List { .. } | SubKey::Search { .. } | SubKey::App | SubKey::Account { .. } => {}
    }
    let gmail = gmail_query(reads, &key)?;
    let online = ctx.state.online();
    if online && let Some((account, query)) = &gmail {
        effects.push(Effect::GmailList {
            sub: sub.clone(),
            account: *account,
            query: query.clone(),
            page: None,
        });
    }
    for effect in effects {
        ctx.state.sub_effect(effect);
    }
    if let Some(replaced) = ctx.state.subs.open(sub, key, online && gmail.is_some()) {
        released(ctx, &replaced);
    }
    Ok(())
}

/// Closes a subscription and cancels its queued Gmail work.
pub(super) fn unsubscribe(ctx: &mut ActionCtx<'_, '_>, sub: SubId) {
    let closed = ctx.state.subs.close(&sub);
    ctx.state.sub_effect(Effect::CancelSub(sub));
    if let Some(key) = closed {
        released(ctx, &key);
    }
}

/// A subscription watching `key` closed: a conversation's thread no other
/// subscription shows no longer needs its bodies read, so a read of them
/// still waiting (offline, backing off) stops.
fn released(ctx: &mut ActionCtx<'_, '_>, key: &SubKey) {
    if let SubKey::Conversation { thread, .. } = *key
        && !ctx.state.subs.watches_thread(thread)
    {
        ctx.state.sub_effect(Effect::ThreadUnneeded(thread));
    }
}

/// Asks Gmail for a list's next page. The page token is taken, so asking
/// again while the page loads does nothing instead of restarting it; the
/// page brings the next token.
pub(super) fn more_results(ctx: &mut ActionCtx<'_, '_>, sub: &SubId) -> Result<(), ActionError> {
    let Some(key) = ctx.state.subs.get_mut(sub).map(|s| s.key.clone()) else {
        return Ok(());
    };
    let Some((account, query)) = gmail_query(ctx.tx.read(), &key)? else {
        return Ok(());
    };
    let Some(page) = ctx.state.subs.get_mut(sub).and_then(|s| s.next_page.take()) else {
        return Ok(());
    };
    ctx.state.sub_effect(Effect::GmailList {
        sub: sub.clone(),
        account,
        query,
        page: Some(page),
    });
    Ok(())
}

/// The Gmail listing a subscription merges in while online: Search, and the
/// Done, All Mail, label, Trash, and Spam views (SPEC Views). A label Gmail
/// has no id for yet has none.
fn gmail_query(
    reads: Reads<'_>,
    key: &SubKey,
) -> Result<Option<(AccountId, GmailQuery)>, StoreError> {
    let (account, listing, typed) = match key {
        SubKey::List { account, view } => (*account, view.gmail_listing(), false),
        SubKey::Search { account, query } => (*account, GmailListing::search(query), true),
        SubKey::App
        | SubKey::Account { .. }
        | SubKey::Conversation { .. }
        | SubKey::Composer { .. } => return Ok(None),
    };
    Ok(match listing {
        None => None,
        Some(GmailListing::Query(q)) if typed => Some((account, GmailQuery::Search(q))),
        Some(GmailListing::Query(q)) => Some((account, GmailQuery::View(q))),
        Some(GmailListing::Label {
            label,
            include_spam_trash,
        }) => reads
            .labels()
            .get(account, &label)?
            .and_then(|l| l.gmail_id)
            .map(|gmail_label| {
                (
                    account,
                    GmailQuery::Label {
                        gmail_label,
                        include_spam_trash,
                    },
                )
            }),
    })
}
