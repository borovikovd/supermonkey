//! First loads and patches, built from the committed snapshot.

use std::collections::HashSet;

use sm_domain::{Millis, SubId, ThreadId, View};
use sm_ipc::{ListEntry, SubKey, SubUpdate};
use sm_store::{Changes, Reads, StoreError};

use super::affects::affects;
use super::diff::diff;
use super::lists::{FIRST_WINDOW, SearchTraits, search_ids, view_ids};
use super::registry::Sub;
use crate::views;

/// What updates are built from.
#[derive(Debug, Clone, Copy)]
pub(super) struct Source<'c> {
    /// The committed snapshot.
    pub(super) reads: Reads<'c>,
    /// Whether Gmail is reachable, for the app subscription.
    pub(super) online: bool,
    /// App time, for searches that name relative dates.
    pub(super) now: Millis,
}

/// A subscription's first load: the whole of what it shows, and for a list
/// its ids and the rows of the top of the window. Records the ids sent.
pub(super) fn first_load(
    source: Source<'_>,
    id: &SubId,
    sub: &mut Sub,
) -> Result<SubUpdate, StoreError> {
    let Source { reads, online, now } = source;
    let sub_id = id.clone();
    Ok(match &sub.key {
        SubKey::List { account, view } => {
            let ids = view_ids(reads, *account, view)?;
            let kind = ListKind::of(view);
            ListUpdate::new(reads, sub_id, sub, ids, kind).first()?
        }
        SubKey::Search { account, query } => {
            let (ids, traits) = search_ids(reads, *account, query, &sub.gmail_hits, now)?;
            let kind = ListKind::Search(traits);
            ListUpdate::new(reads, sub_id, sub, ids, kind).first()?
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
    source: Source<'_>,
    id: &SubId,
    sub: &mut Sub,
    changes: &Changes,
    forced: bool,
) -> Result<Option<SubUpdate>, StoreError> {
    if !forced && !affects(&sub.key, changes) {
        return Ok(None);
    }
    let Source { reads, online, now } = source;
    let sub_id = id.clone();
    Ok(match &sub.key {
        SubKey::List { account, view } => {
            let ids = view_ids(reads, *account, view)?;
            let kind = ListKind::of(view);
            ListUpdate::new(reads, sub_id, sub, ids, kind).patch(changes, forced)?
        }
        SubKey::Search { account, query } => {
            let (ids, traits) = search_ids(reads, *account, query, &sub.gmail_hits, now)?;
            let kind = ListKind::Search(traits);
            ListUpdate::new(reads, sub_id, sub, ids, kind).patch(changes, forced)?
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
    /// Search results, and what the query says about them.
    Search(SearchTraits),
}

impl ListKind {
    fn of(view: &View) -> Self {
        if *view == View::Drafts {
            Self::Drafts
        } else {
            Self::View
        }
    }

    const fn needs_gmail(self) -> bool {
        matches!(self, Self::Search(t) if t.needs_gmail)
    }

    const fn looks_in_trash(self) -> bool {
        matches!(self, Self::Search(t) if t.looks_in_trash)
    }
}

/// A list subscription's update in the making: the list as the snapshot
/// has it now, against what the subscription last sent.
struct ListUpdate<'a, 'c> {
    reads: Reads<'c>,
    sub_id: SubId,
    sub: &'a mut Sub,
    /// The list's entries now.
    ids: Vec<ListEntry>,
    kind: ListKind,
}

impl<'a, 'c> ListUpdate<'a, 'c> {
    const fn new(
        reads: Reads<'c>,
        sub_id: SubId,
        sub: &'a mut Sub,
        ids: Vec<ListEntry>,
        kind: ListKind,
    ) -> Self {
        Self {
            reads,
            sub_id,
            sub,
            ids,
            kind,
        }
    }

    /// The first load: every id, and the rows of the top of the window.
    fn first(self) -> Result<SubUpdate, StoreError> {
        let Self {
            reads,
            sub_id,
            sub,
            ids,
            kind,
        } = self;
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
            looks_in_trash: kind.looks_in_trash(),
            more: sub.next_page.is_some(),
            oldest_hit,
        })
    }

    /// The patch: old ids plus new ids give removals and moves; changed rows
    /// go along whether or not they are in the window. `None` when nothing
    /// changed and the patch is not forced.
    fn patch(self, changes: &Changes, forced: bool) -> Result<Option<SubUpdate>, StoreError> {
        let Self {
            reads,
            sub_id,
            sub,
            ids,
            kind,
        } = self;
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
            looks_in_trash: kind.looks_in_trash(),
            more: sub.next_page.is_some(),
            oldest_hit,
        }))
    }
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
