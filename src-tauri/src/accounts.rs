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
    ctx.tx.execute(
        "INSERT INTO accounts (email, display_name, position, hue) VALUES (?1, ?1, ?2, ?3)",
        params![email, position, hue],
    )?;
    let id = AccountId(ctx.tx.last_insert_rowid());
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
/// sync, deletes its Keychain item, revokes the token, and removes files.
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
