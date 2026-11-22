use sm_domain::{AccountId, GmailListing, SubId};
use sm_ipc::SubKey;
use sm_store::{Reads, StoreError};

use super::{ActionCtx, ActionError};
use crate::drafts::lifecycle;
use crate::effects::{Effect, GmailQuery};

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

pub(super) fn subscribe(
    ctx: &mut ActionCtx<'_, '_>,
    sub: SubId,
    key: SubKey,
) -> Result<(), ActionError> {
    let reads = ctx.tx.read();
    let mut effects = Vec::new();
    match &key {
        SubKey::Conversation { thread, .. } => {
            if let Some(account) = reads.threads().account_of(*thread)? {
                effects.push(Effect::FetchThread {
                    account,
                    thread: *thread,
                });
            }
        }
        SubKey::Composer { draft } => {
            if let Some(record) = reads.drafts().get(draft)?
                && lifecycle::awaits_parts(&record.draft)
            {
                effects.push(Effect::DraftParts {
                    account: record.draft.account,
                    draft: record.draft.id,
                });
            }
        }
        SubKey::List { .. } | SubKey::Search { .. } | SubKey::App | SubKey::Account { .. } => {}
    }
    let gmail = gmail_query(reads, &key)?;
    let online = ctx.state.online();
    let searching = online && gmail.is_some();
    if let Some((account, query)) = gmail {
        effects.push(Effect::GmailList {
            sub: sub.clone(),
            account,
            query,
            page: None,
        });
    }
    for effect in effects {
        ctx.state.sub_effect(effect);
    }
    if let Some(replaced) = ctx.state.subs.open(sub, key, searching) {
        released(ctx, &replaced);
    }
    Ok(())
}

pub(super) fn unsubscribe(ctx: &mut ActionCtx<'_, '_>, sub: SubId) {
    let closed = ctx.state.subs.close(&sub);
    ctx.state.sub_effect(Effect::CancelSub(sub));
    if let Some(key) = closed {
        released(ctx, &key);
    }
}

fn released(ctx: &mut ActionCtx<'_, '_>, key: &SubKey) {
    if let SubKey::Conversation { thread, .. } = *key
        && !ctx.state.subs.watches_thread(thread)
    {
        ctx.state.sub_effect(Effect::ThreadUnneeded(thread));
    }
}

pub(super) fn more_results(ctx: &mut ActionCtx<'_, '_>, sub: SubId) -> Result<(), ActionError> {
    let Some(open) = ctx.state.subs.get(&sub) else {
        return Ok(());
    };
    let Some((account, query)) = gmail_query(ctx.tx.read(), &open.key)? else {
        return Ok(());
    };
    let Some(page) = ctx
        .state
        .subs
        .get_mut(&sub)
        .and_then(|s| s.next_page.take())
    else {
        return Ok(());
    };
    ctx.state.sub_effect(Effect::GmailList {
        sub,
        account,
        query,
        page: Some(page),
    });
    Ok(())
}

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
