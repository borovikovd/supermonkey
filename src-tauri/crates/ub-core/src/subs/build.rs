//! First loads and patches, built from the committed snapshot.

use ub_domain::{SubId, View};
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
) -> Result<SubUpdate, StoreError> {
    let sub_id = id.clone();
    let key = sub.key.clone();
    Ok(match &key {
        SubKey::List { account, view } => {
            let ids = view_ids(reads, *account, view)?;
            list_first(reads, sub_id, sub, ids, *view == View::Drafts)?
        }
        SubKey::Search { account, query } => {
            let ids = search_ids(reads, *account, query, &sub.gmail_hits)?;
            list_first(reads, sub_id, sub, ids, false)?
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
) -> Result<Option<SubUpdate>, StoreError> {
    if !forced && !affects(&sub.key, changes) {
        return Ok(None);
    }
    let sub_id = id.clone();
    let key = sub.key.clone();
    Ok(match &key {
        SubKey::List { account, view } => {
            let ids = view_ids(reads, *account, view)?;
            let drafts = *view == View::Drafts;
            list_patch(reads, sub_id, sub, ids, drafts, changes, forced)?
        }
        SubKey::Search { account, query } => {
            let ids = search_ids(reads, *account, query, &sub.gmail_hits)?;
            list_patch(reads, sub_id, sub, ids, false, changes, forced)?
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

fn list_first(
    reads: Reads<'_>,
    sub_id: SubId,
    sub: &mut Sub,
    ids: Vec<ListEntry>,
    drafts: bool,
) -> Result<SubUpdate, StoreError> {
    let window: Vec<&str> = ids
        .iter()
        .take(FIRST_WINDOW)
        .map(|e| e.key.as_str())
        .collect();
    let rows = views::rows_for_keys(reads, drafts, &window)?;
    sub.ids.clone_from(&ids);
    Ok(SubUpdate::List {
        sub: sub_id,
        first: true,
        ids: Some(ids),
        removed: Vec::new(),
        moved: Vec::new(),
        rows,
        searching: sub.searching,
        more: sub.next_page.is_some(),
    })
}

/// Old ids plus new ids give removals and moves; changed rows go along
/// whether or not they are in the window.
fn list_patch(
    reads: Reads<'_>,
    sub_id: SubId,
    sub: &mut Sub,
    ids: Vec<ListEntry>,
    drafts: bool,
    changes: &Changes,
    forced: bool,
) -> Result<Option<SubUpdate>, StoreError> {
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
    sub.ids = ids;
    Ok(Some(SubUpdate::List {
        sub: sub_id,
        first: false,
        ids: None,
        removed: d.removed,
        moved: d.moved,
        rows,
        searching: sub.searching,
        more: sub.next_page.is_some(),
    }))
}
