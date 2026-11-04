//! Accounts (SPEC Accounts, Settings → Remove account).

use rusqlite::{OptionalExtension, params};

use crate::model::AccountId;
use crate::writer::{Ctx, Effect};

/// Adds an account row (or returns the existing one) with its default splits.
pub fn create(ctx: &mut Ctx<'_>, email: &str) -> anyhow::Result<AccountId> {
    let existing: Option<AccountId> = ctx
        .tx
        .query_row(
            "SELECT id FROM accounts WHERE lower(email) = lower(?1)",
            [email],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        ctx.tx
            .execute("UPDATE accounts SET needs_signin = 0 WHERE id = ?1", [id])?;
        ctx.touch_app();
        ctx.touch_summary(id);
        return Ok(id);
    }
    let (position, hue): (i64, i64) = ctx.tx.query_row(
        "SELECT coalesce(max(position), -1) + 1, coalesce(max(hue), -1) + 1 FROM accounts",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    // Ids are never reused, as AUTOINCREMENT would do (the table predates it
    // and is referenced too widely to rebuild): a removed account's clean-up
    // and leftover tasks act by id and must not reach one added after it.
    let id: AccountId = ctx.tx.query_row(
        "SELECT max(coalesce((SELECT value FROM meta WHERE key = 'account_seq'), 0),
                    coalesce((SELECT max(id) FROM accounts), 0)) + 1",
        [],
        |r| r.get(0),
    )?;
    ctx.tx.execute(
        "INSERT INTO meta (key, value) VALUES ('account_seq', ?1)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [id],
    )?;
    ctx.tx.execute(
        "INSERT INTO accounts (id, email, display_name, position, hue) VALUES (?1, ?2, ?2, ?3, ?4)",
        params![id, email, position, hue],
    )?;
    crate::splits::create_defaults(ctx, id)?;
    for (label, name) in SYSTEM_LABELS {
        ctx.tx.execute(
            "INSERT OR IGNORE INTO labels (account_id, id, gmail_id, name, system) VALUES (?1, ?2, ?2, ?3, 1)",
            params![id, label, name],
        )?;
    }
    ctx.touch_app();
    ctx.touch_summary(id);
    Ok(id)
}

pub const SYSTEM_LABELS: [(&str, &str); 10] = [
    ("INBOX", "Inbox"),
    ("SENT", "Sent"),
    ("DRAFT", "Drafts"),
    ("STARRED", "Starred"),
    ("UNREAD", "Unread"),
    ("IMPORTANT", "Important"),
    ("TRASH", "Trash"),
    ("SPAM", "Spam"),
    ("CATEGORY_PERSONAL", "Personal"),
    ("CATEGORY_PROMOTIONS", "Promotions"),
];

/// Deletes the account's rows, queue, and outbox. The async side stops its
/// sync, deletes its refresh token, revokes it, and removes files.
pub fn remove(ctx: &mut Ctx<'_>, account: AccountId) -> anyhow::Result<()> {
    let email: String =
        ctx.tx
            .query_row("SELECT email FROM accounts WHERE id = ?1", [account], |r| {
                r.get(0)
            })?;
    ctx.tx.execute(
        "DELETE FROM messages_fts WHERE rowid IN (SELECT id FROM messages WHERE account_id = ?1)",
        [account],
    )?;
    ctx.tx
        .execute("DELETE FROM settings WHERE account_id = ?1", [account])?;
    ctx.tx
        .execute("DELETE FROM accounts WHERE id = ?1", [account])?;
    ctx.touched.lists.insert(account);
    ctx.touched.draft_lists.insert(account);
    ctx.touch_summary(account);
    ctx.touch_app();
    ctx.effect(Effect::AccountRemoved { account, email });
    ctx.effect(Effect::Badge);
    Ok(())
}
