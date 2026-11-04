//! The subscription registry: what each open subscription is and what the
//! mirror has of it. It lives on the writer thread, so no lock guards it.

use std::collections::HashMap;

use ub_domain::{SubId, ThreadId};
use ub_gmail::PageToken;
use ub_ipc::{ListEntry, SubKey, SubUpdate};
use ub_store::{Changes, Reads};

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
    /// An id already open is replaced.
    pub(crate) fn open(&mut self, id: SubId, key: SubKey, searching: bool) {
        self.opened = self.opened.saturating_add(1);
        self.subs.insert(
            id,
            Sub {
                key,
                opened: self.opened,
                ids: Vec::new(),
                gmail_hits: Vec::new(),
                searching,
                next_page: None,
                first_sent: false,
            },
        );
    }

    /// Closes a subscription, if it is open.
    pub(crate) fn close(&mut self, id: &SubId) {
        self.subs.remove(id);
    }

    /// Closes every subscription and returns their ids, in open order.
    pub(crate) fn close_all(&mut self) -> Vec<SubId> {
        let mut all: Vec<(SubId, Sub)> = self.subs.drain().collect();
        all.sort_by_key(|(_, sub)| sub.opened);
        all.into_iter().map(|(id, _)| id).collect()
    }

    /// One subscription.
    pub(crate) fn get_mut(&mut self, id: &SubId) -> Option<&mut Sub> {
        self.subs.get_mut(id)
    }

    /// Gives the app subscription a patch with the next commit.
    pub(crate) fn force_app(&mut self) {
        self.app_forced = true;
    }

    /// The updates a commit with `changes` makes, from its snapshot, in the
    /// order the subscriptions opened. Records what each sends, so the next
    /// patch starts from it: call it only once the commit has landed.
    ///
    /// A subscription whose update can't be read gets none now; it gets a
    /// fresh first load with the next commit instead, so the mirror never
    /// keeps a list built on a patch it did not get.
    pub(crate) fn updates(&mut self, reads: Reads<'_>, changes: &Changes, online: bool) -> Updates {
        let app_forced = std::mem::take(&mut self.app_forced);
        let mut subs: Vec<(&SubId, &mut Sub)> = self.subs.iter_mut().collect();
        subs.sort_by_key(|(_, sub)| sub.opened);
        let mut out = Updates::default();
        for (id, sub) in subs {
            if sub.first_sent {
                let forced = app_forced && sub.key == SubKey::App;
                match build::patch(reads, id, sub, changes, forced, online) {
                    Ok(Some(update)) => out.patches.push(update),
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(sub = %id, error = %e, "couldn't build a patch; sending a first load next");
                        sub.first_sent = false;
                    }
                }
            } else {
                match build::first_load(reads, id, sub, online) {
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
