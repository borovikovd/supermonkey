//! Local search (SPEC Search): FTS5 matches plus the Gmail hits for the query.

use rusqlite::{Connection, OptionalExtension};

use crate::model::{AccountId, ListEntry, ThreadId};
use crate::threads::RuleMsg;

/// Most local matches a search list holds.
const LIMIT: i64 = 500;

pub fn ids(
    conn: &Connection,
    account: AccountId,
    query: &str,
    gmail_hits: &[ThreadId],
) -> anyhow::Result<Vec<ListEntry>> {
    let q = ub_rules::Query::parse(query);
    let mut matched: Vec<(i64, ThreadId)> = Vec::new();
    if q.has_local_terms() {
        let fts = q.to_fts5();
        // FTS expresses the whole query: one indexed lookup.
        if let (Some(fts), false) = (&fts, q.needs_post_filter()) {
            let mut stmt = conn.prepare_cached(
                "SELECT t.sort_time, t.id FROM threads t
                     WHERE t.account_id = ?2 AND t.has_live = 1 AND t.id IN (
                       SELECT m.thread_id FROM messages_fts f JOIN messages m ON m.id = f.rowid
                       WHERE messages_fts MATCH ?1 AND m.account_id = ?2 AND m.is_draft = 0)
                     ORDER BY t.sort_time DESC LIMIT ?3",
            )?;
            let rows = stmt.query_map(rusqlite::params![fts, account, LIMIT], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            for r in rows {
                matched.push(r?);
            }
        } else {
            // FTS narrows (or cannot help); the rule language decides.
            let candidates: Vec<ThreadId> = if let Some(fts) = &fts {
                let mut stmt = conn.prepare_cached(
                        "SELECT DISTINCT m.thread_id FROM messages_fts f JOIN messages m ON m.id = f.rowid
                         WHERE messages_fts MATCH ?1 AND m.account_id = ?2 AND m.is_draft = 0 LIMIT 5000",
                    )?;
                stmt.query_map(rusqlite::params![fts, account], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            } else {
                let mut stmt = conn.prepare_cached(
                        "SELECT id FROM threads WHERE account_id = ?1 AND has_live = 1 ORDER BY sort_time DESC LIMIT 5000",
                    )?;
                stmt.query_map([account], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            for t in candidates {
                if let Some(time) = thread_matches(conn, account, t, &q)? {
                    matched.push((time, t));
                }
            }
        }
    }
    for &t in gmail_hits {
        if !matched.iter().any(|(_, x)| *x == t)
            && let Some(time) = conn
                .query_row(
                    "SELECT sort_time FROM threads WHERE id = ?1 AND account_id = ?2 AND has_live = 1",
                    rusqlite::params![t, account],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
        {
            matched.push((time, t));
        }
    }
    matched.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    matched.truncate(usize::try_from(LIMIT).unwrap_or(500).max(gmail_hits.len()));
    Ok(matched
        .into_iter()
        .map(|(_, t)| ListEntry {
            key: t.to_string(),
            split: None,
        })
        .collect())
}

/// The thread's sort time when any live message matches, like Gmail search
/// skips threads with no message outside trash and spam.
pub fn thread_matches(
    conn: &Connection,
    account: AccountId,
    thread: ThreadId,
    q: &ub_rules::Query,
) -> anyhow::Result<Option<i64>> {
    let msgs = crate::threads::messages(conn, thread)?;
    if !msgs.iter().any(crate::threads::Msg::live) {
        return Ok(None);
    }
    let labels = crate::threads::thread_labels(&msgs);
    let user_labels = crate::splits::user_label_names(conn, account, &labels)?;
    let mut body_stmt =
        conn.prepare_cached("SELECT coalesce(body_text, '') FROM messages WHERE id = ?1")?;
    for m in msgs.iter().filter(|m| m.live()) {
        let body: String = body_stmt.query_row([m.id], |r| r.get(0))?;
        let s = RuleMsg {
            msg: m,
            body: &body,
            user_labels: &user_labels,
        };
        if q.matches(&s, ub_rules::Mode::Search) {
            let time = msgs.iter().rev().find(|m| m.live()).map_or(0, |m| m.date);
            return Ok(Some(time));
        }
    }
    Ok(None)
}
