//! Subscriptions (SPEC UI state → Subscriptions). Lives on the writer thread.
//!
//! A list subscription remembers the ids it last sent. After each commit that
//! may change it, the ids are recomputed from the transaction's snapshot and
//! diffed into removals and moves, so the mirror can rebuild the same list.

pub mod diff;
pub mod search;

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::db::queries;
use crate::model::{
    AccountId, AccountInfo, AccountSummary, DraftId, ListEntry, SubId, SubKey, SubUpdate,
    SyncStatus, ThreadId, View,
};
use crate::writer::Touched;

/// Rows sent with a first load, from the top of the list.
pub const FIRST_WINDOW: usize = 80;

pub struct Sub {
    pub key: SubKey,
    pub ids: Vec<ListEntry>,
    /// Search: Gmail hits merged into the local results.
    pub gmail_hits: Vec<ThreadId>,
    pub searching: bool,
    pub next_page: Option<String>,
    pub first_sent: bool,
}

#[derive(Default)]
pub struct Subs {
    pub map: HashMap<SubId, Sub>,
}

impl Subs {
    pub fn subscribe(&mut self, id: SubId, key: SubKey) {
        self.map.insert(
            id,
            Sub {
                key,
                ids: Vec::new(),
                gmail_hits: Vec::new(),
                searching: false,
                next_page: None,
                first_sent: false,
            },
        );
    }

    pub fn unsubscribe(&mut self, id: &str) -> Option<Sub> {
        self.map.remove(id)
    }

    /// Drafts open in a composer.
    pub fn open_drafts(&self) -> HashSet<DraftId> {
        self.map
            .values()
            .filter_map(|s| match &s.key {
                SubKey::Composer { draft } => Some(draft.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn is_draft_open(&self, draft: &str) -> bool {
        self.map
            .values()
            .any(|s| matches!(&s.key, SubKey::Composer { draft: d } if d == draft))
    }

    /// Threads open (not just prefetched) in a conversation.
    pub fn open_threads(&self) -> HashSet<ThreadId> {
        self.map
            .values()
            .filter_map(|s| match &s.key {
                SubKey::Conversation {
                    thread, open: true, ..
                } => Some(*thread),
                _ => None,
            })
            .collect()
    }

    /// First loads for subscriptions that have not had one yet.
    pub fn take_first_loads(
        &mut self,
        conn: &Connection,
        online: bool,
    ) -> anyhow::Result<Vec<SubUpdate>> {
        let mut out = Vec::new();
        for (id, sub) in self.map.iter_mut().filter(|(_, s)| !s.first_sent) {
            sub.first_sent = true;
            out.push(first_load(conn, id, sub, online)?);
        }
        Ok(out)
    }

    /// Patches for every open subscription the commit affects.
    pub fn updates(
        &mut self,
        conn: &Connection,
        t: &Touched,
        online: bool,
    ) -> anyhow::Result<Vec<SubUpdate>> {
        if t.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for (id, sub) in self.map.iter_mut().filter(|(_, s)| s.first_sent) {
            let forced = t.subs.contains(id);
            match sub.key.clone() {
                SubKey::App => {
                    if t.app || forced {
                        out.push(SubUpdate::App {
                            sub: id.clone(),
                            first: false,
                            state: queries::app_state(conn, online)?,
                        });
                    }
                }
                SubKey::Account { account } => {
                    if (t.summaries.contains(&account) || forced)
                        && let Some(summary) = queries::account_summary(conn, account)?
                    {
                        out.push(SubUpdate::Account {
                            sub: id.clone(),
                            first: false,
                            summary,
                        });
                    }
                }
                SubKey::List { account, view } => {
                    let drafts = view == View::Drafts;
                    let hit = if drafts {
                        t.draft_lists.contains(&account)
                    } else {
                        t.lists.contains(&account)
                    };
                    if hit || forced {
                        let ids = queries::list_ids(conn, account, &view)?;
                        out.extend(list_patch(conn, id, sub, ids, t, drafts, forced)?);
                    }
                }
                SubKey::Search { account, query } => {
                    if t.lists.contains(&account) || forced {
                        let ids = search::ids(conn, account, &query, &sub.gmail_hits)?;
                        out.extend(list_patch(conn, id, sub, ids, t, false, forced)?);
                    }
                }
                SubKey::Conversation {
                    thread, trash_mode, ..
                } => {
                    if t.threads.contains(&thread) || forced {
                        out.push(SubUpdate::Conversation {
                            sub: id.clone(),
                            first: false,
                            data: queries::conversation(conn, thread, trash_mode)?,
                        });
                    }
                }
                SubKey::Composer { draft } => {
                    if t.drafts.contains(&draft) || forced {
                        out.push(SubUpdate::Composer {
                            sub: id.clone(),
                            first: false,
                            draft: queries::draft_state(conn, &draft)?,
                        });
                    }
                }
            }
        }
        Ok(out)
    }
}

/// The patch from the last ids sent to `ids`, or None when nothing changed.
/// A forced patch is sent even when empty, so `searching` and `more` reach the mirror.
fn list_patch(
    conn: &Connection,
    id: &str,
    sub: &mut Sub,
    ids: Vec<ListEntry>,
    t: &Touched,
    drafts: bool,
    forced: bool,
) -> anyhow::Result<Option<SubUpdate>> {
    let (removed, moved) = diff::diff(&sub.ids, &ids);
    let changed: Vec<String> = ids
        .iter()
        .filter(|e| {
            if drafts {
                t.drafts.contains(&e.key)
            } else {
                e.key
                    .parse()
                    .is_ok_and(|n: ThreadId| t.threads.contains(&n))
            }
        })
        .map(|e| e.key.clone())
        .collect();
    sub.ids = ids;
    if !forced && removed.is_empty() && moved.is_empty() && changed.is_empty() {
        return Ok(None);
    }
    let rows = queries::rows_for_keys(conn, drafts, &changed)?;
    Ok(Some(SubUpdate::List {
        sub: id.to_string(),
        first: false,
        ids: None,
        removed,
        moved,
        rows,
        searching: sub.searching,
        more: sub.next_page.is_some(),
    }))
}

fn first_load(
    conn: &Connection,
    id: &str,
    sub: &mut Sub,
    online: bool,
) -> anyhow::Result<SubUpdate> {
    Ok(match sub.key.clone() {
        SubKey::App => SubUpdate::App {
            sub: id.to_string(),
            first: true,
            state: queries::app_state(conn, online)?,
        },
        SubKey::Account { account } => SubUpdate::Account {
            sub: id.to_string(),
            first: true,
            summary: queries::account_summary(conn, account)?
                .unwrap_or_else(|| empty_summary(account)),
        },
        SubKey::List { account, view } => {
            let ids = queries::list_ids(conn, account, &view)?;
            list_first(conn, id, sub, ids, view == View::Drafts)?
        }
        SubKey::Search { account, query } => {
            let ids = search::ids(conn, account, &query, &sub.gmail_hits)?;
            list_first(conn, id, sub, ids, false)?
        }
        SubKey::Conversation {
            thread, trash_mode, ..
        } => SubUpdate::Conversation {
            sub: id.to_string(),
            first: true,
            data: queries::conversation(conn, thread, trash_mode)?,
        },
        SubKey::Composer { draft } => SubUpdate::Composer {
            sub: id.to_string(),
            first: true,
            draft: queries::draft_state(conn, &draft)?,
        },
    })
}

fn list_first(
    conn: &Connection,
    id: &str,
    sub: &mut Sub,
    ids: Vec<ListEntry>,
    drafts: bool,
) -> anyhow::Result<SubUpdate> {
    let window: Vec<String> = ids
        .iter()
        .take(FIRST_WINDOW)
        .map(|e| e.key.clone())
        .collect();
    let rows = queries::rows_for_keys(conn, drafts, &window)?;
    sub.ids.clone_from(&ids);
    Ok(SubUpdate::List {
        sub: id.to_string(),
        first: true,
        ids: Some(ids),
        removed: vec![],
        moved: vec![],
        rows,
        searching: sub.searching,
        more: sub.next_page.is_some(),
    })
}

fn empty_summary(account: AccountId) -> AccountSummary {
    AccountSummary {
        account: AccountInfo {
            id: account,
            email: String::new(),
            name: String::new(),
            hue: queries::HUES[0].into(),
            position: 0,
            inbox_unread: 0,
            needs_signin: false,
            muted: false,
            sync_status: SyncStatus::Idle,
            send_as: vec![],
        },
        labels: vec![],
        splits: vec![],
        pins: vec![],
        outbox: vec![],
        undo: vec![],
        notices: vec![],
        queued_ops: 0,
    }
}
