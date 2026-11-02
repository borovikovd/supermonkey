//! Labels (SPEC Labels). A label created here gets the UI's local id; the
//! queued create maps it to the Gmail id when it replays.

use rusqlite::params;

use crate::model::AccountId;
use crate::writer::Ctx;

pub fn create(
    ctx: &mut Ctx<'_>,
    n: i64,
    account: AccountId,
    id: &str,
    name: &str,
) -> anyhow::Result<()> {
    let name = name.trim();
    anyhow::ensure!(!name.is_empty(), "label name is empty");
    // An existing label with that name is used instead.
    let exists: bool = ctx.tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM labels WHERE account_id = ?1 AND (id = ?2 OR lower(name) = lower(?3)))",
        params![account, id, name],
        |r| r.get(0),
    )?;
    if exists {
        return Ok(());
    }
    ctx.tx.execute(
        "INSERT INTO labels (account_id, id, gmail_id, name, system) VALUES (?1, ?2, NULL, ?3, 0)",
        params![account, id, name],
    )?;
    crate::queue::push(
        ctx,
        account,
        Some(n),
        "label_create",
        &serde_json::json!({ "id": id, "name": name }),
    )?;
    ctx.touch_summary(account);
    Ok(())
}
