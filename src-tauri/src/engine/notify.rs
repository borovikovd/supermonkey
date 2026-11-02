//! macOS notifications (SPEC Notifications).

use std::collections::HashSet;

use rusqlite::params;
use tauri_plugin_notification::NotificationExt;

use super::EngineRef;
use crate::db::queries;
use crate::model::AccountId;
use crate::notify::Notification;
use crate::writer::{Ctx, Effect};

/// Notifications for a poll: messages added with `INBOX` and `UNREAD`, not
/// from a connected account, and not in a conversation already on screen.
/// More than three for one account become one summary.
pub fn from_poll(
    ctx: &mut Ctx<'_>,
    account: AccountId,
    new_ids: &HashSet<String>,
) -> anyhow::Result<()> {
    if new_ids.is_empty() {
        return Ok(());
    }
    let muted = queries::setting(ctx.tx, account, "muted")?.as_deref() == Some("on");
    if muted {
        return Ok(());
    }
    let email: String =
        ctx.tx
            .query_row("SELECT email FROM accounts WHERE id = ?1", [account], |r| {
                r.get(0)
            })?;
    // Every connected account's addresses.
    let mut mine: HashSet<String> = HashSet::new();
    let ids: Vec<i64> = {
        let mut stmt = ctx.tx.prepare("SELECT id FROM accounts")?;
        stmt.query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?
    };
    for a in ids {
        mine.extend(queries::my_addresses(ctx.tx, a)?);
    }
    let open = ctx.subs.open_threads();
    let mut out: Vec<Notification> = Vec::new();
    let mut stmt = ctx.tx.prepare_cached(
        "SELECT thread_id, from_name, from_email, subject, snippet, label_ids FROM messages
         WHERE account_id = ?1 AND gmail_id = ?2 AND is_draft = 0",
    )?;
    for gid in new_ids {
        let Ok((thread, name, from, subject, snippet, labels)) =
            stmt.query_row(params![account, gid], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })
        else {
            continue;
        };
        let labels: Vec<String> = crate::db::from_json(&labels);
        let has = |l: &str| labels.iter().any(|x| x == l);
        if !has("INBOX")
            || !has("UNREAD")
            || mine.contains(&from.to_lowercase())
            || open.contains(&thread)
        {
            continue;
        }
        out.push(Notification {
            account,
            account_email: email.clone(),
            thread: Some(thread),
            title: if name.is_empty() { from } else { name },
            body: if subject.is_empty() {
                snippet
            } else {
                format!("{subject}\n{snippet}")
            },
        });
    }
    if out.len() > 3 {
        ctx.effect(Effect::Notify(Notification {
            account,
            account_email: email.clone(),
            thread: None,
            title: format!("{} new messages", out.len()),
            body: email,
        }));
    } else {
        for n in out {
            ctx.effect(Effect::Notify(n));
        }
    }
    Ok(())
}

pub fn show(e: &EngineRef, n: Notification) {
    *e.last_notification.lock() = Some((n.clone(), crate::db::now_ms()));
    let Some(app) = &e.app else { return };
    let _ = app
        .notification()
        .builder()
        .title(&n.title)
        .body(format!("{} · {}", n.account_email, n.body))
        .show();
}
