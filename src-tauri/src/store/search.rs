use std::collections::HashMap;

use rusqlite::params;

use crate::domain::query::{MatchTarget, Query, Scope};
use crate::domain::{
    AccountId, Addr, Identity, LabelId, MessageFacts, Millis, SystemLabel, ThreadId,
};

use super::sql::{id_list, json_col};
use super::{CANDIDATE_LIMIT, Db, SEARCH_LIMIT, StoreError};

impl Db<'_> {
    pub fn search_threads(
        self,
        account: AccountId,
        query: &Query,
        gmail_hits: &[ThreadId],
    ) -> Result<Vec<ThreadId>, StoreError> {
        let mut by_id: HashMap<ThreadId, Millis> = HashMap::new();
        if query.searches_locally() && !query.is_empty() {
            for (tid, time) in self.search_local(account, query)? {
                by_id.insert(tid, time);
            }
        }
        if !gmail_hits.is_empty() {
            let encoded = id_list(gmail_hits)?;
            let scope = query.scope();
            let list_requirement = query.list_requirement();
            let extra = self.all(
                "SELECT id, sort_time, has_live, EXISTS (SELECT 1 FROM messages m WHERE m.thread_id = threads.id AND m.is_list = 1) FROM threads WHERE account_id = ?1 AND id IN (SELECT value FROM json_each(?2))",
                params![account, encoded],
                "load gmail search hits",
                |r| {
                    Ok((
                        r.get::<_, ThreadId>(0)?,
                        r.get::<_, Millis>(1)?,
                        r.get::<_, i64>(2)? != 0,
                        r.get::<_, i64>(3)? != 0,
                    ))
                },
            )?;
            for (tid, time, has_live, from_list) in extra {
                let in_scope =
                    has_live || matches!(scope, Scope::Trash | Scope::Spam | Scope::Anywhere);
                if in_scope && list_requirement.is_none_or(|wanted| from_list == wanted) {
                    by_id.insert(tid, time);
                }
            }
        }
        let mut sorted: Vec<(ThreadId, Millis)> = by_id.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
        Ok(sorted.into_iter().map(|(tid, _)| tid).collect())
    }

    fn search_local(
        self,
        account: AccountId,
        query: &Query,
    ) -> Result<Vec<(ThreadId, Millis)>, StoreError> {
        let fts = query.to_fts5();
        if !query.needs_post_filter()
            && let Some(fts_expr) = &fts
        {
            return self.search_fts_only(account, fts_expr);
        }
        let scope = query.scope();
        let candidates = self.search_candidates(account, fts.as_deref(), scope)?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let Some(acct) = self.account(account)? else {
            return Ok(Vec::new());
        };
        let identity = acct.identity();
        let user_labels = self.user_label_names(account)?;
        let needs_body = query.reads_body();
        let mut matched = Vec::new();
        for (tid, sort_time, snoozed) in candidates {
            if self.thread_matches_search(
                tid,
                query,
                scope,
                &user_labels,
                &identity,
                snoozed,
                needs_body,
            )? {
                matched.push((tid, sort_time));
                if matched.len() >= SEARCH_LIMIT {
                    break;
                }
            }
        }
        Ok(matched)
    }

    fn search_fts_only(
        self,
        account: AccountId,
        fts: &str,
    ) -> Result<Vec<(ThreadId, Millis)>, StoreError> {
        let limit = i64::try_from(SEARCH_LIMIT).unwrap_or(500);
        self.all(
            "SELECT DISTINCT t.id, t.sort_time FROM messages_fts f
             JOIN messages m ON m.id = f.rowid
             JOIN threads t ON t.id = m.thread_id
             WHERE f.messages_fts MATCH ?2 AND m.account_id = ?1 AND m.is_live = 1 AND t.has_live = 1
             ORDER BY t.sort_time DESC, t.id DESC LIMIT ?3",
            params![account, fts, limit],
            "fts search threads",
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    }

    fn search_candidates(
        self,
        account: AccountId,
        fts: Option<&str>,
        scope: Scope,
    ) -> Result<Vec<(ThreadId, Millis, bool)>, StoreError> {
        let limit = i64::try_from(CANDIDATE_LIMIT).unwrap_or(5000);
        let live_filter = if matches!(scope, Scope::Normal) {
            "AND t.has_live = 1"
        } else {
            ""
        };
        if let Some(fts_expr) = fts {
            let sql = format!(
                "SELECT DISTINCT t.id, t.sort_time, t.snooze_until IS NOT NULL
                 FROM messages_fts f
                 JOIN messages m ON m.id = f.rowid
                 JOIN threads t ON t.id = m.thread_id
                 WHERE f.messages_fts MATCH ?2 AND m.account_id = ?1 {live_filter}
                 ORDER BY t.sort_time DESC, t.id DESC LIMIT ?3"
            );
            self.all(
                &sql,
                params![account, fts_expr, limit],
                "fts candidate threads",
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
            )
        } else {
            let sql = format!(
                "SELECT t.id, t.sort_time, t.snooze_until IS NOT NULL
                 FROM threads t WHERE t.account_id = ?1 {live_filter}
                 ORDER BY t.sort_time DESC, t.id DESC LIMIT ?2"
            );
            self.all(
                &sql,
                params![account, limit],
                "scan candidate threads",
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
            )
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "search matching combines thread metadata, query AST, scope, and account context"
    )]
    fn thread_matches_search(
        self,
        tid: ThreadId,
        query: &Query,
        scope: Scope,
        user_labels: &HashMap<LabelId, String>,
        identity: &Identity,
        snoozed: bool,
        needs_body: bool,
    ) -> Result<bool, StoreError> {
        let rows = self.load_search_messages(tid, needs_body)?;
        for (msg, body) in rows {
            if !admits_message(&msg, scope) {
                continue;
            }
            let target = MatchTarget::search(&msg, &body, user_labels, identity, snoozed);
            if query.matches(&target) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn load_search_messages(
        self,
        tid: ThreadId,
        needs_body: bool,
    ) -> Result<Vec<(MessageFacts, String)>, StoreError> {
        let body_col = if needs_body {
            "COALESCE(body_text, snippet)"
        } else {
            "''"
        };
        let sql = format!(
            "SELECT id, gmail_id, internal_date, from_name, from_email, to_addrs, cc_addrs, subject, snippet, is_list, is_draft, has_attachments, label_ids, {body_col}
             FROM messages WHERE thread_id = ?1"
        );
        self.all(&sql, [tid], "load search messages", |r| {
            let labels: Vec<LabelId> = json_col(r, 12)?;
            let facts = MessageFacts {
                id: r.get(0)?,
                gmail_id: r.get(1)?,
                date: r.get(2)?,
                from: Addr {
                    name: r.get(3)?,
                    email: r.get(4)?,
                },
                to: json_col(r, 5)?,
                cc: json_col(r, 6)?,
                bcc: Vec::new(),
                reply_to: Vec::new(),
                subject: r.get(7)?,
                snippet: r.get(8)?,
                message_id_hdr: None,
                in_reply_to: None,
                references: Vec::new(),
                is_list: r.get::<_, i64>(9)? != 0,
                is_draft: r.get::<_, i64>(10)? != 0,
                has_attachments: r.get::<_, i64>(11)? != 0,
                labels: labels.into_iter().collect(),
            };
            Ok((facts, r.get(13)?))
        })
    }
}

fn admits_message(msg: &MessageFacts, scope: Scope) -> bool {
    match scope {
        Scope::Normal => msg.is_live(),
        Scope::Trash => !msg.is_draft && msg.has(SystemLabel::Trash),
        Scope::Spam => !msg.is_draft && msg.has(SystemLabel::Spam),
        Scope::Drafts => msg.is_draft,
        Scope::Anywhere => !msg.is_draft,
    }
}
