//! A list subscription's ids, in order (SPEC Views, Search).

use chrono::{DateTime, Local};
use ub_domain::{AccountId, Millis, Query, ThreadId, View};
use ub_ipc::ListEntry;
use ub_store::{Reads, StoreError, Table};

/// Rows a list's first load carries, from the top.
pub(crate) const FIRST_WINDOW: usize = 80;

/// What [`view_ids`] reads of a view's account, besides the Drafts list:
/// thread rows and their labels, and the split order the Inbox sorts by.
/// Its changed rows come from the thread ids a commit wrote.
pub(crate) const VIEW_READS: &[Table] = &[Table::Threads, Table::Splits];

/// What [`view_ids`] reads for the Drafts list: drafts without an outbox
/// row. Its changed rows come from the draft ids a commit wrote.
pub(crate) const DRAFTS_READS: &[Table] = &[Table::Drafts, Table::Outbox];

/// What [`search_ids`] reads of its account: thread rows and their messages
/// (a message write re-derives its thread), the account's addresses for
/// `from:me`, and the label names `label:` matches.
pub(crate) const SEARCH_READS: &[Table] = &[Table::Threads, Table::Account, Table::Labels];

/// A view's keys in list order: thread ids with their split, or draft ids
/// for the Drafts list.
pub(crate) fn view_ids(
    reads: Reads<'_>,
    account: AccountId,
    view: &View,
) -> Result<Vec<ListEntry>, StoreError> {
    if *view == View::Drafts {
        return Ok(reads
            .drafts()
            .list(account)?
            .into_iter()
            .map(|draft| ListEntry {
                key: draft.to_string(),
                split: None,
            })
            .collect());
    }
    Ok(reads
        .threads()
        .list(account, view)?
        .into_iter()
        .map(|listed| ListEntry {
            key: listed.thread.to_string(),
            split: listed.split,
        })
        .collect())
}

/// A search's keys: local matches plus the Gmail hits it holds, newest
/// first; and whether the query needs Gmail, because local search can't
/// evaluate one of its terms. Dates in the query are days in this Mac's time
/// zone, and ages count back from `now`.
pub(crate) fn search_ids(
    reads: Reads<'_>,
    account: AccountId,
    raw: &str,
    gmail_hits: &[ThreadId],
    now: Millis,
) -> Result<(Vec<ListEntry>, bool), StoreError> {
    let query = DateTime::from_timestamp_millis(now.0).map_or_else(
        || Query::parse(raw),
        |now| Query::parse_search(raw, &now.with_timezone(&Local)),
    );
    let ids = reads
        .search()
        .threads(account, &query, gmail_hits)?
        .into_iter()
        .map(|thread| ListEntry {
            key: thread.to_string(),
            split: None,
        })
        .collect();
    Ok((ids, query.needs_gmail()))
}
