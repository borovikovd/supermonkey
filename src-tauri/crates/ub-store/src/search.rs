//! Local search (SPEC Search): the FTS5 index narrows, the query language
//! decides, and Gmail's hits are merged in.

use std::collections::HashSet;

use ub_domain::query::MatchTarget;
use ub_domain::{AccountId, Millis, Query, Thread, ThreadId};

use crate::StoreError;
use crate::db::Db;
use crate::labels::LabelReads;
use crate::messages::MessageReads;

/// Most local matches a search lists. Gmail hits are always listed, even
/// past it.
pub const SEARCH_LIMIT: usize = 500;

/// Most threads checked one by one when the index can't decide alone.
const CANDIDATE_LIMIT: i64 = 5_000;

/// Local search.
#[derive(Clone, Copy)]
pub struct SearchReads<'c> {
    db: Db<'c>,
}

impl<'c> SearchReads<'c> {
    pub(crate) fn new(db: Db<'c>) -> Self {
        Self { db }
    }

    /// The threads a Search list holds, newest first: local matches (a live
    /// message matches the query), plus the Gmail hits for it that have a
    /// live message. Like Gmail search, threads with no message outside
    /// Trash and Spam are left out.
    pub fn threads(
        self,
        account: AccountId,
        query: &Query,
        gmail_hits: &[ThreadId],
    ) -> Result<Vec<ThreadId>, StoreError> {
        let mut matched = if query.has_local_terms() {
            self.local(account, query)?
        } else {
            Vec::new()
        };
        let mut seen: HashSet<ThreadId> = matched.iter().map(|&(_, t)| t).collect();
        for &hit in gmail_hits {
            if seen.contains(&hit) {
                continue;
            }
            let time: Option<Millis> = self.db.optional(
                "read a Gmail hit's time",
                "SELECT sort_time FROM threads WHERE id = ?1 AND account_id = ?2 AND has_live = 1",
                (hit, account),
                |r| r.get("sort_time"),
            )?;
            if let Some(time) = time {
                seen.insert(hit);
                matched.push((time, hit));
            }
        }
        matched.sort_unstable_by(|a, b| b.cmp(a));
        matched.truncate(SEARCH_LIMIT.max(gmail_hits.len()));
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
            // The index expresses the whole query: one lookup.
            return self.db.all(
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
            );
        }
        // The index narrows, or can't help; the query language decides.
        let candidates: Vec<ThreadId> = match &fts {
            Some(fts) => self.db.all(
                "narrow a search with the index",
                "SELECT DISTINCT m.thread_id FROM messages_fts f JOIN messages m ON m.id = f.rowid
                 WHERE messages_fts MATCH ?1 AND m.account_id = ?2 AND m.is_live = 1
                 LIMIT ?3",
                (fts, account, CANDIDATE_LIMIT),
                |r| r.get("thread_id"),
            )?,
            None => self.db.all(
                "read search candidates",
                "SELECT id FROM threads WHERE account_id = ?1 AND has_live = 1
                 ORDER BY sort_time DESC, id DESC LIMIT ?2",
                (account, CANDIDATE_LIMIT),
                |r| r.get("id"),
            )?,
        };
        let names = LabelReads::new(self.db).user_label_names(account)?;
        let messages = MessageReads::new(self.db);
        let mut matched = Vec::new();
        for id in candidates {
            let thread = Thread::new(messages.facts(id)?);
            let label_names: Vec<String> = thread
                .labels()
                .iter()
                .filter_map(|l| names.get(l).cloned())
                .collect();
            for m in thread.messages().iter().filter(|m| m.is_live()) {
                let body = if query.reads_body() {
                    messages.body_text(m.id)?
                } else {
                    String::new()
                };
                if query.matches(&MatchTarget::search(m, &label_names, &body)) {
                    if let Some(latest) = thread.latest() {
                        matched.push((latest.date, id));
                    }
                    break;
                }
            }
        }
        Ok(matched)
    }
}
