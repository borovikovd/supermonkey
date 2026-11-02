//! Recipient autocomplete (SPEC Recipients and autocomplete).

use std::sync::Arc;

use rusqlite::{Connection, params};
use ub_gmail::{Format, GmailError, Priority, ThreadListQuery, cost};

use super::{AccountRt, EngineRef};
use crate::model::{AccountId, Addr, ContactSuggestion};

/// Up to `limit` suggestions, ranked: sent-to first (by sent count weighted
/// toward recent contact), then received-from, then co-recipients.
pub fn suggest(
    conn: &Connection,
    account: AccountId,
    query: &str,
    limit: usize,
) -> anyhow::Result<Vec<ContactSuggestion>> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let like = format!("{}%", q.replace(['%', '_'], ""));
    let mut stmt = conn.prepare_cached(
        "SELECT email, name, sent_count, recv_count, list_only, last_contact FROM contacts
         WHERE account_id = ?1 AND (email LIKE ?2 OR email LIKE '%@' || ?2 OR name LIKE ?2 OR name LIKE '% ' || ?2
                                    OR name LIKE '%-' || ?2 OR email LIKE '%.' || ?2)
         LIMIT 2000",
    )?;
    let mine = crate::db::queries::my_addresses(conn, account)?;
    let now = crate::db::now_ms();
    let mut out: Vec<(u8, f64, ContactSuggestion)> = Vec::new();
    let rows = stmt.query_map(params![account, like], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, u32>(2)?,
            r.get::<_, u32>(3)?,
            r.get::<_, bool>(4)?,
            r.get::<_, i64>(5)?,
        ))
    })?;
    for row in rows {
        let (email, name, sent, recv, list_only, last) = row?;
        let own = mine.contains(&email);
        if !ub_rules::is_suggestible(sent, list_only, ub_rules::is_noreply(&email), own) {
            continue;
        }
        let Some(span) = ub_rules::contact_matches(&q, &name, &email) else {
            continue;
        };
        let (tier, score) = ub_rules::contact_score(sent, recv, last, now);
        out.push((
            tier,
            score,
            ContactSuggestion {
                name,
                email,
                match_field: match span.field {
                    ub_rules::MatchField::Name => "name".into(),
                    ub_rules::MatchField::Email => "email".into(),
                },
                match_start: span.start as u32,
                match_end: span.end as u32,
            },
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
    Ok(out.into_iter().take(limit).map(|x| x.2).collect())
}

/// One page of the contacts backfill: the To, Cc, and Date headers of the last
/// year of sent mail, at priority 6.
pub async fn backfill_page(
    e: &EngineRef,
    rt: &Arc<AccountRt>,
    page: Option<String>,
) -> Result<(), GmailError> {
    let q = ThreadListQuery {
        q: Some("in:sent newer_than:1y".into()),
        page_token: page,
        max_results: Some(100),
        ..Default::default()
    };
    let res = rt
        .sched
        .run(Priority::P6, cost::THREADS_LIST, rt.gmail.list_threads(&q))
        .await?;
    for t in res.threads {
        let thread = rt
            .sched
            .run(
                Priority::P6,
                cost::threads_get(Format::Metadata),
                rt.gmail
                    .get_thread(&t.id, Format::Metadata, &["From", "To", "Cc", "Date"]),
            )
            .await;
        let thread = match thread {
            Ok(t) => t,
            Err(err) if matches!(err.outcome, ub_gmail::Outcome::NotFound) => continue,
            Err(err) => return Err(err),
        };
        let msgs: Vec<_> = thread
            .messages
            .iter()
            .filter(|m| {
                m.label_ids.iter().any(|l| l == "SENT") && !m.label_ids.iter().any(|l| l == "DRAFT")
            })
            .map(|m| super::ingest::parse(m, false, &std::collections::HashMap::default()))
            .collect();
        let account = rt.id;
        e.writer.sync(move |ctx| {
            let mine = crate::db::queries::my_addresses(ctx.tx, account)?;
            for m in &msgs {
                let from = m.from.clone().unwrap_or(Addr {
                    name: None,
                    email: String::new(),
                });
                super::ingest::count_contacts(ctx, account, m, &from, &mine)?;
            }
            Ok(())
        });
    }
    if let Some(next) = res.next_page_token {
        rt.work.push(
            super::work::BACKFILL,
            super::work::Work::Backfill { page: Some(next) },
        );
    }
    Ok(())
}
