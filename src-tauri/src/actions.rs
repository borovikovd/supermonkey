//! Applies one numbered UI action inside a writer transaction.

use rusqlite::params;

use crate::model::*;
use crate::writer::{Ctx, Effect, GmailListQuery};

pub fn apply(ctx: &mut Ctx, n: i64, action: Action) -> anyhow::Result<()> {
    match action {
        Action::Reset => {
            // A reloaded page: close every subscription of the previous page.
            // Open composers close as a second Esc would.
            for draft in ctx.subs.open_drafts() {
                crate::drafts::close(ctx, &draft)?;
            }
            let ids: Vec<SubId> = ctx.subs.map.keys().cloned().collect();
            for id in ids {
                ctx.subs.unsubscribe(&id);
                ctx.effect(Effect::CancelSub(id));
            }
            Ok(())
        }
        Action::Subscribe { sub, key } => subscribe(ctx, sub, key),
        Action::Unsubscribe { sub } => {
            ctx.subs.unsubscribe(&sub);
            ctx.effect(Effect::CancelSub(sub));
            Ok(())
        }
        Action::Modify(m) => crate::mail::modify(ctx, n, m),
        Action::Undo { account, entry } => crate::undo::undo(ctx, n, account, entry),
        Action::CreateLabel {
            account,
            id,
            name,
            apply,
        } => {
            crate::labels::create(ctx, n, account, &id, &name)?;
            if let Some(m) = apply {
                crate::mail::modify(ctx, n, m)?;
            }
            Ok(())
        }
        Action::SaveDraft { draft, save } => crate::drafts::save(ctx, &draft, save),
        Action::CloseComposer { draft } => {
            crate::drafts::close(ctx, &draft)?;
            if let Some(id) = composer_sub(ctx, &draft) {
                ctx.subs.unsubscribe(&id);
            }
            Ok(())
        }
        Action::SendDraft { draft } => {
            crate::outbox::send(ctx, n, &draft)?;
            if let Some(id) = composer_sub(ctx, &draft) {
                ctx.subs.unsubscribe(&id);
            }
            Ok(())
        }
        Action::DiscardDraft { draft } => {
            crate::drafts::discard(ctx, &draft)?;
            if let Some(id) = composer_sub(ctx, &draft) {
                ctx.subs.unsubscribe(&id);
            }
            Ok(())
        }
        Action::CloseToast { outbox } => crate::outbox::close_toast(ctx, outbox),
        Action::AckNotice { notice } => {
            let account: Option<i64> = ctx
                .tx
                .query_row(
                    "SELECT account_id FROM notices WHERE id = ?1",
                    [notice],
                    |r| r.get(0),
                )
                .ok();
            ctx.tx
                .execute("DELETE FROM notices WHERE id = ?1", [notice])?;
            if let Some(a) = account {
                ctx.touch_summary(a);
            }
            Ok(())
        }
        Action::SetSetting {
            account,
            key,
            value,
        } => {
            let acc = account.unwrap_or(0);
            ctx.tx.execute(
                "INSERT INTO settings (account_id, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT (account_id, key) DO UPDATE SET value = excluded.value",
                params![acc, key, value],
            )?;
            ctx.touch_app();
            if acc != 0 {
                ctx.touch_summary(acc);
            }
            if key == "muted" {
                ctx.effect(Effect::Badge);
            }
            Ok(())
        }
        Action::Pin {
            account,
            kind,
            value,
            split,
        } => crate::splits::pin(ctx, n, account, kind, &value, split),
        Action::SaveSplits {
            account,
            splits,
            deleted,
        } => crate::splits::save(ctx, account, splits, deleted),
        Action::RemoveAccount { account } => crate::accounts::remove(ctx, account),
        Action::MoreResults { sub } => {
            if let Some(s) = ctx.subs.map.get(&sub)
                && let Some(token) = s.next_page.clone()
                && let Some((account, query)) = gmail_query(ctx, &s.key)
            {
                ctx.effect(Effect::GmailList {
                    sub,
                    account,
                    query,
                    page_token: Some(token),
                });
            }
            Ok(())
        }
    }
}

fn composer_sub(ctx: &Ctx, draft: &str) -> Option<SubId> {
    ctx.subs
        .map
        .iter()
        .find(|(_, s)| matches!(&s.key, SubKey::Composer { draft: d } if d == draft))
        .map(|(id, _)| id.clone())
}

/// The Gmail listing a subscription needs while online: Search, and the
/// Label, Trash, and Spam views (SPEC Views).
fn gmail_query(ctx: &Ctx, key: &SubKey) -> Option<(AccountId, GmailListQuery)> {
    match key {
        SubKey::Search { account, query } if !query.trim().is_empty() => Some((
            *account,
            GmailListQuery::Search(ub_rules::strip_is_list(query)),
        )),
        SubKey::List { account, view } => {
            let (label, spam_trash) = match view {
                View::Trash => ("TRASH".to_string(), true),
                View::Spam => ("SPAM".to_string(), true),
                View::Label { label } => (label.clone(), false),
                _ => return None,
            };
            let gmail_id: String = ctx
                .tx
                .query_row(
                    "SELECT gmail_id FROM labels WHERE account_id = ?1 AND id = ?2",
                    params![account, label],
                    |r| r.get(0),
                )
                .ok()?;
            Some((
                *account,
                GmailListQuery::Label {
                    label_gmail_id: gmail_id,
                    include_spam_trash: spam_trash,
                },
            ))
        }
        _ => None,
    }
}

fn subscribe(ctx: &mut Ctx, sub: SubId, key: SubKey) -> anyhow::Result<()> {
    match &key {
        SubKey::Conversation { thread, open, .. } => {
            if let Some(account) = crate::threads::account_of(ctx.tx, *thread)? {
                ctx.effect(Effect::FetchThread {
                    account,
                    thread: *thread,
                    open: *open,
                });
                if *open {
                    ctx.effect(Effect::OnScreen(account));
                }
            }
        }
        SubKey::Composer { draft } => {
            let needs: Option<(i64, bool)> = ctx
                .tx
                .query_row(
                    "SELECT account_id, needs_parts FROM drafts WHERE id = ?1",
                    [draft],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .ok();
            if let Some((account, true)) = needs {
                ctx.effect(Effect::DraftParts {
                    account,
                    draft: draft.clone(),
                });
            }
        }
        SubKey::List { account, .. } | SubKey::Search { account, .. } => {
            ctx.effect(Effect::OnScreen(*account));
        }
        _ => {}
    }
    let gmail = gmail_query(ctx, &key);
    ctx.subs.subscribe(sub.clone(), key);
    if let Some((account, query)) = gmail {
        if let Some(s) = ctx.subs.map.get_mut(&sub) {
            s.searching = ctx.online;
        }
        if ctx.online {
            ctx.effect(Effect::GmailList {
                sub,
                account,
                query,
                page_token: None,
            });
        }
    }
    Ok(())
}
