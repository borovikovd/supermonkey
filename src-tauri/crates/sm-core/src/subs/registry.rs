//! The subscription registry: what each open subscription is and what the
//! mirror has of it. It lives on the writer thread, so no lock guards it.

use std::collections::HashMap;

use sm_domain::{DraftId, Millis, SubId, ThreadId};
use sm_gmail::PageToken;
use sm_ipc::{ListEntry, SubKey, SubUpdate};
use sm_store::{Changes, Reads};

use super::build;

/// One open subscription.
#[derive(Debug)]
pub(crate) struct Sub {
    /// What it watches.
    pub(crate) key: SubKey,
    /// Open order, so updates go out in the order the UI subscribed.
    opened: u64,
    /// A list's ids as the mirror has them: the last ones sent.
    pub(super) ids: Vec<ListEntry>,
    /// Search and Gmail-backed views: the Gmail hits merged into its rows.
    pub(crate) gmail_hits: Vec<ThreadId>,
    /// A Gmail listing for it is running.
    pub(crate) searching: bool,
    /// Gmail's next page, while it has one; taken by `moreResults`.
    pub(crate) next_page: Option<PageToken>,
    /// Its first load went out; until then it gets no patches.
    first_sent: bool,
    /// It gets a patch with the next commit, whatever that changed: its
    /// `searching` or `more` changed, which live here, not in the database.
    forced: bool,
}

/// A commit's updates for the subscriptions: patches, then the first loads
/// of subscriptions that have not had one, each its own channel message.
#[derive(Debug, Default)]
pub(crate) struct Updates {
    /// Patches for subscriptions whose first load went out earlier.
    pub(crate) patches: Vec<SubUpdate>,
    /// First loads.
    pub(crate) first_loads: Vec<SubUpdate>,
}

/// Every open subscription, by id.
#[derive(Debug, Default)]
pub(crate) struct Registry {
    subs: HashMap<SubId, Sub>,
    opened: u64,
    /// The app subscription gets a patch with the next commit, whatever it
    /// changed: the online flag lives here, not in the database.
    app_forced: bool,
}

impl Registry {
    /// Opens a subscription; its first load goes out with the next commit.
    /// An id already open is replaced; returns what it watched.
    pub(crate) fn open(&mut self, id: SubId, key: SubKey, searching: bool) -> Option<SubKey> {
        self.opened = self.opened.saturating_add(1);
        self.subs
            .insert(
                id,
                Sub {
                    key,
                    opened: self.opened,
                    ids: Vec::new(),
                    gmail_hits: Vec::new(),
                    searching,
                    next_page: None,
                    first_sent: false,
                    forced: false,
                },
            )
            .map(|replaced| replaced.key)
    }

    /// Closes a subscription, if it is open, and returns what it watched.
    pub(crate) fn close(&mut self, id: &SubId) -> Option<SubKey> {
        self.subs.remove(id).map(|sub| sub.key)
    }

    /// Closes every subscription and returns their ids and what they
    /// watched, in open order.
    pub(crate) fn close_all(&mut self) -> Vec<(SubId, SubKey)> {
        let mut all: Vec<(SubId, Sub)> = self.subs.drain().collect();
        all.sort_by_key(|(_, sub)| sub.opened);
        all.into_iter().map(|(id, sub)| (id, sub.key)).collect()
    }

    /// Whether a conversation, open or prefetched, shows `thread`: its
    /// bodies are still wanted.
    pub(crate) fn watches_thread(&self, thread: ThreadId) -> bool {
        self.subs
            .values()
            .any(|s| matches!(&s.key, SubKey::Conversation { thread: t, .. } if *t == thread))
    }

    /// Whether a composer has `draft` open (SPEC Drafts: Rust treats a
    /// draft open in a composer differently).
    pub(crate) fn has_composer(&self, draft: &DraftId) -> bool {
        self.subs.values().any(|s| is_composer_of(&s.key, draft))
    }

    /// Whether a conversation shows `thread` open, not just prefetched.
    pub(crate) fn is_open(&self, thread: ThreadId) -> bool {
        self.subs.values().any(|s| {
            matches!(&s.key, SubKey::Conversation { thread: t, open: true, .. } if *t == thread)
        })
    }

    /// The drafts open in composers, in open order.
    pub(crate) fn composer_drafts(&self) -> Vec<DraftId> {
        let mut open: Vec<(&Sub, &DraftId)> = self
            .subs
            .values()
            .filter_map(|s| match &s.key {
                SubKey::Composer { draft } => Some((s, draft)),
                _ => None,
            })
            .collect();
        open.sort_by_key(|(s, _)| s.opened);
        open.into_iter().map(|(_, d)| d.clone()).collect()
    }

    /// Closes the composers of `draft`: it was sent, discarded, or closed.
    pub(crate) fn close_composers(&mut self, draft: &DraftId) {
        self.subs.retain(|_, s| !is_composer_of(&s.key, draft));
    }

    /// One subscription.
    pub(crate) fn get(&self, id: &SubId) -> Option<&Sub> {
        self.subs.get(id)
    }

    /// One subscription, to change.
    pub(crate) fn get_mut(&mut self, id: &SubId) -> Option<&mut Sub> {
        self.subs.get_mut(id)
    }

    /// Gives the app subscription a patch with the next commit.
    pub(crate) fn force_app(&mut self) {
        self.app_forced = true;
    }

    /// A list subscription's Gmail listing waits for Gmail, or asks it: the
    /// list shows `searching` only while it asks. A subscription closed
    /// meanwhile is left alone.
    pub(crate) fn set_searching(&mut self, id: &SubId, searching: bool) {
        if let Some(sub) = self.subs.get_mut(id)
            && sub.searching != searching
        {
            sub.searching = searching;
            sub.forced = true;
        }
    }

    /// A Gmail listing page for a list subscription finished: its hits join
    /// the ones it holds, it offers `next`, and it stops searching. The
    /// subscription gets a patch with the next commit. A subscription closed
    /// meanwhile is left alone.
    pub(crate) fn merge_gmail_hits(
        &mut self,
        id: &SubId,
        hits: Vec<ThreadId>,
        next: Option<PageToken>,
    ) {
        let Some(sub) = self.subs.get_mut(id) else {
            return;
        };
        for hit in hits {
            if !sub.gmail_hits.contains(&hit) {
                sub.gmail_hits.push(hit);
            }
        }
        sub.next_page = next;
        sub.searching = false;
        sub.forced = true;
    }

    /// The updates a commit with `changes` makes, from its snapshot, in the
    /// order the subscriptions opened. Records what each sends, so the next
    /// patch starts from it: call it only once the commit has landed.
    ///
    /// A subscription whose update can't be read gets none now; it gets a
    /// fresh first load with the next commit instead, so the mirror never
    /// keeps a list built on a patch it did not get.
    pub(crate) fn updates(
        &mut self,
        reads: Reads<'_>,
        changes: &Changes,
        online: bool,
        now: Millis,
    ) -> Updates {
        let app_forced = std::mem::take(&mut self.app_forced);
        let mut subs: Vec<(&SubId, &mut Sub)> = self.subs.iter_mut().collect();
        subs.sort_by_key(|(_, sub)| sub.opened);
        let mut out = Updates::default();
        for (id, sub) in subs {
            let forced = std::mem::take(&mut sub.forced) || (app_forced && sub.key == SubKey::App);
            if sub.first_sent {
                match build::patch(reads, id, sub, changes, forced, online, now) {
                    Ok(Some(update)) => out.patches.push(update),
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(sub = %id, error = %e, "couldn't build a patch; sending a first load next");
                        sub.first_sent = false;
                    }
                }
            } else {
                match build::first_load(reads, id, sub, online, now) {
                    Ok(update) => {
                        sub.first_sent = true;
                        out.first_loads.push(update);
                    }
                    Err(e) => {
                        tracing::warn!(sub = %id, error = %e, "couldn't build a first load; trying again next commit");
                    }
                }
            }
        }
        out
    }
}

fn is_composer_of(key: &SubKey, draft: &DraftId) -> bool {
    matches!(key, SubKey::Composer { draft: d } if d == draft)
}
