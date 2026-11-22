use std::collections::HashSet;

use sm_domain::{Millis, SubId, ThreadId, View};
use sm_ipc::{ListEntry, SubKey, SubUpdate};
use sm_store::{Changes, Reads, StoreError};

use super::affects::affects;
use super::diff::{ListDiff, diff};
use super::lists::{FIRST_WINDOW, SearchTraits, search_ids, view_ids};
use super::registry::Sub;
use crate::views;

#[derive(Debug, Clone, Copy)]
pub(super) struct Source<'c> {
    pub(super) reads: Reads<'c>,
    pub(super) online: bool,
    pub(super) now: Millis,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum ListKind {
    View { in_trash: bool },
    Drafts,
    Search(SearchTraits),
}

impl ListKind {
    const fn of(view: &View) -> Self {
        match view {
            View::Drafts => Self::Drafts,
            View::Trash | View::Spam => Self::View { in_trash: true },
            View::Inbox
            | View::Done
            | View::AllMail
            | View::Snoozed
            | View::Starred
            | View::Sent
            | View::Label { .. } => Self::View { in_trash: false },
        }
    }

    const fn needs_gmail(&self) -> bool {
        matches!(self, Self::Search(t) if t.needs_gmail)
    }

    const fn looks_in_trash(&self) -> bool {
        match self {
            Self::View { in_trash } => *in_trash,
            Self::Search(t) => t.looks_in_trash,
            Self::Drafts => false,
        }
    }

    fn into_terms(self) -> Vec<String> {
        match self {
            Self::Search(t) => t.terms,
            Self::View { .. } | Self::Drafts => Vec::new(),
        }
    }
}

struct ListUpdate<'a, 'c> {
    reads: Reads<'c>,
    sub_id: SubId,
    sub: &'a mut Sub,
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
            terms: kind.into_terms(),
            more: sub.next_page.is_some(),
            oldest_hit,
        })
    }

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
        let ListDiff {
            removed,
            moved,
            changed,
        } = d;
        let rows = views::rows_for_keys(reads, drafts, &changed)?;
        let oldest_hit = oldest_hit(&ids, &sub.gmail_hits);
        sub.ids = ids;
        Ok(Some(SubUpdate::List {
            sub: sub_id,
            first: false,
            ids: None,
            removed,
            moved,
            rows,
            searching: sub.searching,
            needs_gmail: kind.needs_gmail(),
            looks_in_trash: kind.looks_in_trash(),
            terms: kind.into_terms(),
            more: sub.next_page.is_some(),
            oldest_hit,
        }))
    }
}

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
