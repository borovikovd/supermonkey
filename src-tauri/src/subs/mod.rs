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

/// A commit's messages for the subscriptions, and what they record once it
/// lands. Nothing is recorded before then, so a failed commit leaves each
/// subscription as it was: a first load that never went out goes out later.
#[derive(Default)]
pub struct Outgoing {
    updates: Vec<SubUpdate>,
    /// The list ids each list subscription sent.
    ids: Vec<(SubId, Vec<ListEntry>)>,
    /// Subscriptions whose first load this is.
    first: Vec<SubId>,
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

    pub fn unsubscribe(&mut self, id: &SubId) -> Option<Sub> {
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

    pub fn is_draft_open(&self, draft: &DraftId) -> bool {
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

    /// Patches for every open subscription the commit affects, then first
    /// loads for those that have not had one, all from the commit's snapshot.
    /// Apply the result with [`Subs::sent`] once the commit has landed.
    pub fn outgoing(
        &self,
        conn: &Connection,
        t: &Touched,
        online: bool,
    ) -> anyhow::Result<Outgoing> {
        let mut out = Outgoing::default();
        self.updates(conn, t, online, &mut out)?;
        for (id, sub) in self.map.iter().filter(|(_, s)| !s.first_sent) {
            out.first.push(id.clone());
            out.updates
                .push(first_load(conn, id, sub, online, &mut out.ids)?);
        }
        Ok(out)
    }

    /// Records what a committed [`Outgoing`] sent and returns its messages.
    pub fn sent(&mut self, out: Outgoing) -> Vec<SubUpdate> {
        for (id, ids) in out.ids {
            if let Some(sub) = self.map.get_mut(&id) {
                sub.ids = ids;
            }
        }
        for id in out.first {
            if let Some(sub) = self.map.get_mut(&id) {
                sub.first_sent = true;
            }
        }
        out.updates
    }

    fn updates(
        &self,
        conn: &Connection,
        t: &Touched,
        online: bool,
        out: &mut Outgoing,
    ) -> anyhow::Result<()> {
        if t.is_empty() {
            return Ok(());
        }
        let out_ids = &mut out.ids;
        let out = &mut out.updates;
        for (id, sub) in self.map.iter().filter(|(_, s)| s.first_sent) {
            let forced = t.subs.contains(id);
            match &sub.key {
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
                    if (t.summaries.contains(account) || forced)
                        && let Some(summary) = queries::account_summary(conn, *account)?
                    {
                        out.push(SubUpdate::Account {
                            sub: id.clone(),
                            first: false,
                            summary,
                        });
                    }
                }
                SubKey::List { account, view } => {
                    let drafts = *view == View::Drafts;
                    let hit = if drafts {
                        t.draft_lists.contains(account)
                    } else {
                        t.lists.contains(account)
                    };
                    if hit || forced {
                        let ids = queries::list_ids(conn, *account, view)?;
                        out.extend(list_patch(conn, id, sub, &ids, t, drafts, forced)?);
                        out_ids.push((id.clone(), ids));
                    }
                }
                SubKey::Search { account, query } => {
                    if t.lists.contains(account) || forced {
                        let ids = search::ids(conn, *account, query, &sub.gmail_hits)?;
                        out.extend(list_patch(conn, id, sub, &ids, t, false, forced)?);
                        out_ids.push((id.clone(), ids));
                    }
                }
                SubKey::Conversation {
                    thread, trash_mode, ..
                } => {
                    if t.threads.contains(thread) || forced {
                        out.push(SubUpdate::Conversation {
                            sub: id.clone(),
                            first: false,
                            data: queries::conversation(conn, *thread, *trash_mode)?,
                        });
                    }
                }
                SubKey::Composer { draft } => {
                    if t.drafts.contains(draft) || forced {
                        out.push(SubUpdate::Composer {
                            sub: id.clone(),
                            first: false,
                            draft: queries::draft_state(conn, draft)?,
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

/// The patch from the last ids sent to `ids`, or None when nothing changed.
/// A forced patch is sent even when empty, so `searching` and `more` reach the mirror.
fn list_patch(
    conn: &Connection,
    id: &SubId,
    sub: &Sub,
    ids: &[ListEntry],
    t: &Touched,
    drafts: bool,
    forced: bool,
) -> anyhow::Result<Option<SubUpdate>> {
    let (removed, moved) = diff::diff(&sub.ids, ids);
    let changed: Vec<&str> = ids
        .iter()
        .filter(|e| {
            if drafts {
                t.drafts.contains(e.key.as_str())
            } else {
                e.key
                    .parse()
                    .is_ok_and(|n: ThreadId| t.threads.contains(&n))
            }
        })
        .map(|e| e.key.as_str())
        .collect();
    if !forced && removed.is_empty() && moved.is_empty() && changed.is_empty() {
        return Ok(None);
    }
    let rows = queries::rows_for_keys(conn, drafts, &changed)?;
    Ok(Some(SubUpdate::List {
        sub: id.clone(),
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
    id: &SubId,
    sub: &Sub,
    online: bool,
    sent_ids: &mut Vec<(SubId, Vec<ListEntry>)>,
) -> anyhow::Result<SubUpdate> {
    Ok(match &sub.key {
        SubKey::App => SubUpdate::App {
            sub: id.clone(),
            first: true,
            state: queries::app_state(conn, online)?,
        },
        SubKey::Account { account } => SubUpdate::Account {
            sub: id.clone(),
            first: true,
            summary: queries::account_summary(conn, *account)?
                .unwrap_or_else(|| empty_summary(*account)),
        },
        SubKey::List { account, view } => {
            let drafts = *view == View::Drafts;
            let ids = queries::list_ids(conn, *account, view)?;
            list_first(conn, id, sub, ids, drafts, sent_ids)?
        }
        SubKey::Search { account, query } => {
            let ids = search::ids(conn, *account, query, &sub.gmail_hits)?;
            list_first(conn, id, sub, ids, false, sent_ids)?
        }
        SubKey::Conversation {
            thread, trash_mode, ..
        } => SubUpdate::Conversation {
            sub: id.clone(),
            first: true,
            data: queries::conversation(conn, *thread, *trash_mode)?,
        },
        SubKey::Composer { draft } => SubUpdate::Composer {
            sub: id.clone(),
            first: true,
            draft: queries::draft_state(conn, draft)?,
        },
    })
}

fn list_first(
    conn: &Connection,
    id: &SubId,
    sub: &Sub,
    ids: Vec<ListEntry>,
    drafts: bool,
    sent_ids: &mut Vec<(SubId, Vec<ListEntry>)>,
) -> anyhow::Result<SubUpdate> {
    let window: Vec<&str> = ids
        .iter()
        .take(FIRST_WINDOW)
        .map(|e| e.key.as_str())
        .collect();
    let rows = queries::rows_for_keys(conn, drafts, &window)?;
    sent_ids.push((id.clone(), ids.clone()));
    Ok(SubUpdate::List {
        sub: id.clone(),
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
