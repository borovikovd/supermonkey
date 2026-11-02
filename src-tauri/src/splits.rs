//! Split Inbox routing and editing (SPEC Split Inbox).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

use rusqlite::{Connection, OptionalExtension, params};

use crate::model::{AccountId, PinKind, SplitId, SplitInfo, SplitKind, ThreadId};
use crate::threads::{Msg, RuleMsg};
use crate::writer::Ctx;

thread_local! {
    /// Parsed custom split queries, by query text.
    static PARSED: RefCell<HashMap<String, ub_rules::Query>> = RefCell::new(HashMap::new());
}

/// Names of the user labels among a thread's labels.
pub fn user_label_names(
    conn: &Connection,
    account: AccountId,
    labels: &BTreeSet<String>,
) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT name FROM labels WHERE account_id = ?1 AND id = ?2 AND system = 0",
    )?;
    let mut out = Vec::new();
    for l in labels {
        if let Some(name) = stmt
            .query_row(params![account, l], |r| r.get::<_, String>(0))
            .optional()?
        {
            out.push(name);
        }
    }
    Ok(out)
}

struct SplitRow {
    id: SplitId,
    kind: SplitKind,
    query: String,
    enabled: bool,
}

/// The split an Inbox thread lands in. First match wins: an enabled pin
/// (sender, then domain, then label), enabled custom splits top to bottom,
/// Important for person-to-person mail, then Other.
pub fn route(
    conn: &Connection,
    account: AccountId,
    labels: &BTreeSet<String>,
    msg: &Msg,
) -> anyhow::Result<Option<SplitId>> {
    let splits: Vec<SplitRow> = {
        let mut stmt = conn.prepare_cached(
            "SELECT id, kind, query, enabled FROM splits WHERE account_id = ?1 ORDER BY position, id",
        )?;
        stmt.query_map([account], |r| {
            Ok(SplitRow {
                id: r.get(0)?,
                kind: SplitKind::parse(&r.get::<_, String>(1)?),
                query: r.get(2)?,
                enabled: r.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?
    };
    let enabled = |id: SplitId| splits.iter().any(|s| s.id == id && s.enabled);
    let order = |id: SplitId| splits.iter().position(|s| s.id == id).unwrap_or(usize::MAX);
    let user_labels = user_label_names(conn, account, labels)?;

    let pin = |kind: PinKind, value: &str| -> anyhow::Result<Option<SplitId>> {
        Ok(conn
            .prepare_cached("SELECT split_id FROM split_overrides WHERE account_id = ?1 AND kind = ?2 AND value = ?3")?
            .query_row(params![account, kind.as_str(), value], |r| r.get::<_, i64>(0))
            .optional()?
            .filter(|&id| enabled(id)))
    };
    let email = msg.from_email.to_lowercase();
    if let Some(id) = pin(PinKind::Sender, &email)? {
        return Ok(Some(id));
    }
    if let Some(domain) = email.split('@').nth(1)
        && let Some(id) = pin(PinKind::Domain, domain)?
    {
        return Ok(Some(id));
    }
    let mut label_hits: Vec<SplitId> = Vec::new();
    for name in &user_labels {
        if let Some(id) = pin(PinKind::Label, &name.to_lowercase())? {
            label_hits.push(id);
        }
    }
    if let Some(id) = label_hits.into_iter().min_by_key(|&id| order(id)) {
        return Ok(Some(id));
    }

    let subject = RuleMsg {
        msg,
        body: "",
        user_labels: &user_labels,
    };
    for s in splits
        .iter()
        .filter(|s| s.kind == SplitKind::Custom && s.enabled && !s.query.trim().is_empty())
    {
        // Look up before inserting so a cache hit does not copy the query.
        let hit = PARSED.with_borrow_mut(|cache| {
            if !cache.contains_key(&s.query) {
                cache.insert(s.query.clone(), ub_rules::Query::parse(&s.query));
            }
            cache
                .get(&s.query)
                .is_some_and(|q| q.matches(&subject, ub_rules::Mode::Rule))
        });
        if hit {
            return Ok(Some(s.id));
        }
    }
    let labels: Vec<&str> = msg.labels.iter().map(String::as_str).collect();
    if let Some(imp) = splits
        .iter()
        .find(|s| s.kind == SplitKind::Important && s.enabled)
        && ub_rules::is_person_to_person(&labels, msg.is_list)
    {
        return Ok(Some(imp.id));
    }
    Ok(splits
        .iter()
        .find(|s| s.kind == SplitKind::Other)
        .map(|s| s.id))
}

/// Creates Important and Other for a new account.
pub fn create_defaults(ctx: &mut Ctx<'_>, account: AccountId) -> anyhow::Result<()> {
    ctx.tx.execute(
        "INSERT INTO splits (account_id, name, position, kind, hide_when_empty) VALUES
           (?1, 'Important', 900000, 'important', 1), (?1, 'Other', 1000000, 'other', 0)",
        [account],
    )?;
    Ok(())
}

/// Re-routes every Inbox thread of the account, stamping the ones that move.
pub fn reroute_inbox(ctx: &mut Ctx<'_>, account: AccountId) -> anyhow::Result<()> {
    let ids: Vec<(ThreadId, Option<SplitId>)> = {
        let mut stmt = ctx
            .tx
            .prepare("SELECT id, split_id FROM threads WHERE account_id = ?1 AND in_inbox = 1")?;
        stmt.query_map([account], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?
    };
    let mine = crate::db::queries::my_addresses(ctx.tx, account)?;
    for (thread, old) in ids {
        let msgs = crate::threads::messages(ctx.tx, thread)?;
        let labels = crate::threads::thread_labels(&msgs);
        let Some(m) = crate::threads::latest_from_someone_else(&msgs, &mine) else {
            continue;
        };
        let new = route(ctx.tx, account, &labels, m)?;
        if new != old {
            ctx.tx.execute(
                "UPDATE threads SET split_id = ?2 WHERE id = ?1",
                params![thread, new],
            )?;
            ctx.touch_thread(account, thread)?;
        }
    }
    ctx.touched.lists.insert(account);
    ctx.touch_summary(account);
    Ok(())
}

/// `Add to split`, `Move to Important/Other`: pins a sender, domain, or label.
/// The undo entry remembers the split it replaced.
pub fn pin(
    ctx: &mut Ctx<'_>,
    n: i64,
    account: AccountId,
    kind: PinKind,
    value: &str,
    split: SplitId,
) -> anyhow::Result<()> {
    let value = value.trim().to_lowercase();
    anyhow::ensure!(!value.is_empty(), "empty pin");
    let replaced: Option<SplitId> = ctx
        .tx
        .query_row(
            "SELECT split_id FROM split_overrides WHERE account_id = ?1 AND kind = ?2 AND value = ?3",
            params![account, kind.as_str(), value],
            |r| r.get(0),
        )
        .optional()?;
    ctx.tx.execute(
        "INSERT INTO split_overrides (account_id, kind, value, split_id) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (account_id, kind, value) DO UPDATE SET split_id = excluded.split_id",
        params![account, kind.as_str(), value, split],
    )?;
    let name: String = ctx
        .tx
        .query_row("SELECT name FROM splits WHERE id = ?1", [split], |r| {
            r.get(0)
        })
        .unwrap_or_default();
    crate::undo::record(
        ctx,
        n,
        account,
        "pin",
        &format!("Moved to {name}"),
        &serde_json::json!({ "kind": kind.as_str(), "value": value, "split": split, "replaced": replaced }),
    )?;
    reroute_inbox(ctx, account)
}

pub fn undo_pin(
    ctx: &mut Ctx<'_>,
    account: AccountId,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let kind = payload["kind"].as_str().unwrap_or("sender");
    let value = payload["value"].as_str().unwrap_or_default();
    match payload["replaced"].as_i64() {
        Some(prev) => {
            ctx.tx.execute(
                "UPDATE split_overrides SET split_id = ?4 WHERE account_id = ?1 AND kind = ?2 AND value = ?3",
                params![account, kind, value, prev],
            )?;
        }
        None => {
            ctx.tx.execute(
                "DELETE FROM split_overrides WHERE account_id = ?1 AND kind = ?2 AND value = ?3",
                params![account, kind, value],
            )?;
        }
    }
    reroute_inbox(ctx, account)
}

/// The settings sheet's save: add, rename, reorder, turn off, delete custom splits.
pub fn save(
    ctx: &mut Ctx<'_>,
    account: AccountId,
    splits: &[SplitInfo],
    deleted: Vec<SplitId>,
) -> anyhow::Result<()> {
    for id in deleted {
        // Important and Other cannot be deleted.
        ctx.tx.execute(
            "DELETE FROM splits WHERE id = ?1 AND account_id = ?2 AND kind = 'custom'",
            params![id, account],
        )?;
    }
    // Custom splits come first, then Important, then Other.
    let mut custom_index = 0i64;
    for s in splits {
        let position = match s.kind {
            SplitKind::Other => 1_000_000,
            SplitKind::Important => 900_000,
            SplitKind::Custom => {
                custom_index += 1;
                custom_index * 10
            }
        };
        if s.kind == SplitKind::Custom && !s.query.trim().is_empty() {
            ub_rules::Query::parse_strict(&s.query)
                .map_err(|e| anyhow::anyhow!("{}: {e}", s.name))?;
        }
        if s.id > 0 {
            ctx.tx.execute(
                "UPDATE splits SET name = ?3, position = ?4, query = ?5, hide_when_empty = ?6,
                    enabled = CASE kind WHEN 'other' THEN 1 ELSE ?7 END
                 WHERE id = ?1 AND account_id = ?2",
                params![
                    s.id,
                    account,
                    s.name.trim(),
                    position,
                    s.query,
                    s.hide_when_empty,
                    s.enabled
                ],
            )?;
        } else if s.kind == SplitKind::Custom {
            ctx.tx.execute(
                "INSERT INTO splits (account_id, name, position, query, hide_when_empty, enabled, kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'custom')",
                params![account, s.name.trim(), position, s.query, s.hide_when_empty, s.enabled],
            )?;
        }
    }
    reroute_inbox(ctx, account)
}
