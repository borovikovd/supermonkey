//! First loads and patches, built from the committed snapshot.

use std::collections::HashSet;

use ub_domain::{Millis, SubId, ThreadId, View};
use ub_ipc::{ListEntry, SubKey, SubUpdate};
use ub_store::{Changes, Reads, StoreError};

use super::affects::affects;
use super::diff::diff;
use super::lists::{FIRST_WINDOW, search_ids, view_ids};
use super::registry::Sub;
use crate::views;

/// A subscription's first load: the whole of what it shows, and for a list
/// its ids and the rows of the top of the window. Records the ids sent.
pub(super) fn first_load(
    reads: Reads<'_>,
    id: &SubId,
    sub: &mut Sub,
    online: bool,
    now: Millis,
) -> Result<SubUpdate, StoreError> {
    let sub_id = id.clone();
    Ok(match &sub.key {
        SubKey::List { account, view } => {
            let ids = view_ids(reads, *account, view)?;
            let kind = ListKind::of(view);
            list_first(reads, sub_id, sub, ids, kind)?
        }
        SubKey::Search { account, query } => {
            let (ids, needs_gmail) = search_ids(reads, *account, query, &sub.gmail_hits, now)?;
            list_first(reads, sub_id, sub, ids, ListKind::Search { needs_gmail })?
        }
        SubKey::App => SubUpdate::App {
            sub: sub_id,
            first: true,
            state: views::app_state(reads, online)?,
        },
        SubKey::Account { account } => SubUpdate::Account {
            sub: sub_id,
            first: true,
            summary: views::account_summary(reads, *account)?
                .unwrap_or_else(|| views::empty_summary(*account)),
        },
        SubKey::Conversation {
            thread, trash_mode, ..
        } => SubUpdate::Conversation {
            sub: sub_id,
            first: true,
            data: views::conversation(reads, *thread, *trash_mode)?,
        },
        SubKey::Composer { draft } => SubUpdate::Composer {
            sub: sub_id,
            first: true,
            draft: views::draft_state(reads, draft)?,
        },
    })
}

/// The patch for a subscription the commit may have changed, or `None` when
/// it did not change it. A forced patch goes out even when it changes
/// nothing, so the app's `online` and a list's `searching` and `more` reach
/// the mirror. Records the ids sent.
pub(super) fn patch(
    reads: Reads<'_>,
    id: &SubId,
    sub: &mut Sub,
    changes: &Changes,
    forced: bool,
    online: bool,
    now: Millis,
) -> Result<Option<SubUpdate>, StoreError> {
    if !forced && !affects(&sub.key, changes) {
        return Ok(None);
    }
    let sub_id = id.clone();
    Ok(match &sub.key {
        SubKey::List { account, view } => {
            let ids = view_ids(reads, *account, view)?;
            let kind = ListKind::of(view);
            list_patch(reads, sub_id, sub, ids, kind, changes, forced)?
        }
        SubKey::Search { account, query } => {
            let (ids, needs_gmail) = search_ids(reads, *account, query, &sub.gmail_hits, now)?;
            let kind = ListKind::Search { needs_gmail };
            list_patch(reads, sub_id, sub, ids, kind, changes, forced)?
        }
        SubKey::App => Some(SubUpdate::App {
            sub: sub_id,
            first: false,
            state: views::app_state(reads, online)?,
        }),
        // Patches start once the account exists.
        SubKey::Account { account } => {
            views::account_summary(reads, *account)?.map(|summary| SubUpdate::Account {
                sub: sub_id,
                first: false,
                summary,
            })
        }
        SubKey::Conversation {
            thread, trash_mode, ..
        } => Some(SubUpdate::Conversation {
            sub: sub_id,
            first: false,
            data: views::conversation(reads, *thread, *trash_mode)?,
        }),
        SubKey::Composer { draft } => Some(SubUpdate::Composer {
            sub: sub_id,
            first: false,
            draft: views::draft_state(reads, draft)?,
        }),
    })
}

/// What a list holds, as its updates need to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListKind {
    /// Threads in a view.
    View,
    /// Drafts, keyed by draft id.
    Drafts,
    /// Search results, and whether the query needs Gmail.
    Search { needs_gmail: bool },
}

impl ListKind {
    fn of(view: &View) -> Self {
        if *view == View::Drafts {
            Self::Drafts
        } else {
            Self::View
        }
    }

    fn needs_gmail(self) -> bool {
        matches!(self, Self::Search { needs_gmail: true })
    }
}

fn list_first(
    reads: Reads<'_>,
    sub_id: SubId,
    sub: &mut Sub,
    ids: Vec<ListEntry>,
    kind: ListKind,
) -> Result<SubUpdate, StoreError> {
    let drafts = kind == ListKind::Drafts;
    let window: Vec<&str> = ids
        .iter()
        .take(FIRST_WINDOW)
        .map(|e| e.key.as_str())
        .collect();
    let rows = views::rows_for_keys(reads, drafts, &window)?;
    let oldest_hit = oldest_hit(&ids, &sub.gmail_hits);
    sub.ids.clone_from(&ids);
    Ok(SubUpdate::List {
        sub: sub_id,
        first: true,
        ids: Some(ids),
        removed: Vec::new(),
        moved: Vec::new(),
        rows,
        searching: sub.searching,
        needs_gmail: kind.needs_gmail(),
        more: sub.next_page.is_some(),
        oldest_hit,
    })
}

/// Old ids plus new ids give removals and moves; changed rows go along
/// whether or not they are in the window.
fn list_patch(
    reads: Reads<'_>,
    sub_id: SubId,
    sub: &mut Sub,
    ids: Vec<ListEntry>,
    kind: ListKind,
    changes: &Changes,
    forced: bool,
) -> Result<Option<SubUpdate>, StoreError> {
    let drafts = kind == ListKind::Drafts;
    let d = diff(&sub.ids, &ids, |key| {
        if drafts {
            changes.drafts.contains(key)
        } else {
            key.parse().is_ok_and(|t| changes.threads.contains(&t))
        }
    });
    if !forced && d.is_empty() {
        return Ok(None);
    }
    let rows = views::rows_for_keys(reads, drafts, &d.changed)?;
    let oldest_hit = oldest_hit(&ids, &sub.gmail_hits);
    sub.ids = ids;
    Ok(Some(SubUpdate::List {
        sub: sub_id,
        first: false,
        ids: None,
        removed: d.removed,
        moved: d.moved,
        rows,
        searching: sub.searching,
        needs_gmail: kind.needs_gmail(),
        more: sub.next_page.is_some(),
        oldest_hit,
    }))
}

/// The key of the last entry that is one of the list's Gmail hits.
fn oldest_hit(ids: &[ListEntry], hits: &[ThreadId]) -> Option<String> {
    if hits.is_empty() {
        return None;
    }
    let hits: HashSet<&ThreadId> = hits.iter().collect();
    ids.iter()
        .rev()
        .find(|e| e.key.parse().is_ok_and(|t: ThreadId| hits.contains(&t)))
        .map(|e| e.key.clone())
}
