use chrono::{DateTime, Local};
use sm_domain::{AccountId, Millis, Query, ThreadId, View};
use sm_ipc::ListEntry;
use sm_store::{Reads, StoreError, Table};

pub(crate) const FIRST_WINDOW: usize = 80;

pub(crate) const VIEW_READS: &[Table] = &[Table::Threads, Table::Splits];

pub(crate) const DRAFTS_READS: &[Table] = &[Table::Drafts, Table::Outbox];

pub(crate) const SEARCH_READS: &[Table] = &[Table::Threads, Table::Account, Table::Labels];

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchTraits {
    pub(crate) needs_gmail: bool,
    pub(crate) looks_in_trash: bool,
    pub(crate) terms: Vec<String>,
}

pub(crate) fn search_ids(
    reads: Reads<'_>,
    account: AccountId,
    raw: &str,
    gmail_hits: &[ThreadId],
    now: Millis,
) -> Result<(Vec<ListEntry>, SearchTraits), StoreError> {
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
    let scope = query.scope();
    Ok((
        ids,
        SearchTraits {
            needs_gmail: query.needs_gmail(),
            looks_in_trash: scope.trash || scope.spam,
            terms: query.free_text(),
        },
    ))
}
