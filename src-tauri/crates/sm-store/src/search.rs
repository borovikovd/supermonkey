use std::collections::{BTreeSet, HashSet};

use sm_domain::query::MatchTarget;
use sm_domain::{AccountId, MessageFacts, Millis, Query, Thread, ThreadId};

use crate::StoreError;
use crate::accounts::AccountReads;
use crate::db::Db;
use crate::labels::LabelReads;
use crate::messages::MessageReads;

pub const SEARCH_LIMIT: usize = 500;

const CANDIDATE_LIMIT: i64 = 5_000;

#[derive(Debug, Clone, Copy)]
pub struct SearchReads<'c> {
    db: Db<'c>,
}

impl<'c> SearchReads<'c> {
    pub(crate) const fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    pub fn threads(
        self,
        account: AccountId,
        query: &Query,
        gmail_hits: &[ThreadId],
    ) -> Result<Vec<ThreadId>, StoreError> {
        let mut matched = if query.searches_locally() {
            self.local(account, query)?
        } else {
            Vec::new()
        };
        matched.sort_unstable_by(|a, b| b.cmp(a));
        matched.truncate(SEARCH_LIMIT);
        let past_live = !query.scope().live_only();
        let mut seen: HashSet<ThreadId> = matched.iter().map(|&(_, t)| t).collect();
        for &hit in gmail_hits {
            if seen.contains(&hit) {
                continue;
            }
            let time: Option<Millis> = self.db.optional(
                "read a Gmail hit's time",
                "SELECT sort_time FROM threads
                 WHERE id = ?1 AND account_id = ?2 AND (has_live = 1 OR ?3)",
                (hit, account, past_live),
                |r| r.get("sort_time"),
            )?;
            if let Some(time) = time {
                seen.insert(hit);
                matched.push((time, hit));
            }
        }
        matched.sort_unstable_by(|a, b| b.cmp(a));
        Ok(matched.into_iter().map(|(_, t)| t).collect())
    }

    fn local(
        self,
        account: AccountId,
        query: &Query,
    ) -> Result<Vec<(Millis, ThreadId)>, StoreError> {
        let fts = query.to_fts5();
        if let Some(fts) = &fts
            && !query.needs_post_filter()
        {
            return self.indexed(account, fts);
        }
        let scope = query.scope();
        let past_live = !scope.live_only();
        let candidates = self.candidates(account, fts.as_deref(), past_live)?;
        let Some(me) = AccountReads::new(self.db).identity(account)? else {
            return Ok(Vec::new());
        };
        let names = LabelReads::new(self.db).user_label_names(account)?;
        let messages = MessageReads::new(self.db);
        let mut matched = Vec::new();
        for (id, snoozed) in candidates {
            let thread = Thread::new(messages.facts(id)?);
            let looked_at: Vec<&MessageFacts> = thread
                .messages()
                .iter()
                .filter(|m| scope.admits(m))
                .collect();
            let label_names: Vec<&str> = looked_at
                .iter()
                .flat_map(|m| m.labels.iter())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .filter_map(|l| names.get(l).map(String::as_str))
                .collect();
            for m in looked_at {
                let body = if query.reads_body() {
                    messages.body_text(m.id)?
                } else {
                    String::new()
                };
                let target = MatchTarget::search(m, &label_names, &body, snoozed, &me);
                if query.matches(&target) {
                    if let Some(latest) = thread.latest() {
                        matched.push((latest.date, id));
                    }
                    break;
                }
            }
        }
        Ok(matched)
    }

    fn indexed(self, account: AccountId, fts: &str) -> Result<Vec<(Millis, ThreadId)>, StoreError> {
        self.db.all(
            "search the index",
            "SELECT t.id, t.sort_time FROM threads t
             WHERE t.account_id = ?2 AND t.has_live = 1 AND t.id IN (
               SELECT m.thread_id FROM messages_fts f JOIN messages m ON m.id = f.rowid
               WHERE messages_fts MATCH ?1 AND m.account_id = ?2 AND m.is_live = 1)
             ORDER BY t.sort_time DESC, t.id DESC LIMIT ?3",
            (
                fts,
                account,
                i64::try_from(SEARCH_LIMIT).unwrap_or(i64::MAX),
            ),
            |r| Ok((r.get("sort_time")?, r.get("id")?)),
        )
    }

    fn candidates(
        self,
        account: AccountId,
        fts: Option<&str>,
        past_live: bool,
    ) -> Result<Vec<(ThreadId, bool)>, StoreError> {
        match fts {
            Some(fts) => self.db.all(
                "narrow a search with the index",
                "SELECT t.id, t.snooze_until IS NOT NULL AS snoozed FROM threads t
                 WHERE t.account_id = ?2 AND (t.has_live = 1 OR ?4) AND t.id IN (
                   SELECT m.thread_id FROM messages_fts f JOIN messages m ON m.id = f.rowid
                   WHERE messages_fts MATCH ?1 AND m.account_id = ?2 AND (m.is_live = 1 OR ?4))
                 LIMIT ?3",
                (fts, account, CANDIDATE_LIMIT, past_live),
                |r| Ok((r.get("id")?, r.get("snoozed")?)),
            ),
            None => self.db.all(
                "read search candidates",
                "SELECT id, snooze_until IS NOT NULL AS snoozed FROM threads
                 WHERE account_id = ?1 AND (has_live = 1 OR ?3)
                 ORDER BY sort_time DESC, id DESC LIMIT ?2",
                (account, CANDIDATE_LIMIT, past_live),
                |r| Ok((r.get("id")?, r.get("snoozed")?)),
            ),
        }
    }
}
