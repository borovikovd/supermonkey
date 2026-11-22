use std::collections::HashMap;

use sm_domain::{DraftId, Millis, SubId, ThreadId};
use sm_gmail::PageToken;
use sm_ipc::{ListEntry, SubKey, SubUpdate};
use sm_store::{Changes, Reads};

use super::build::{self, Source};

#[derive(Debug)]
pub(crate) struct Sub {
    pub(crate) key: SubKey,
    opened: u64,
    pub(super) ids: Vec<ListEntry>,
    pub(crate) gmail_hits: Vec<ThreadId>,
    pub(crate) searching: bool,
    pub(crate) next_page: Option<PageToken>,
    first_sent: bool,
    forced: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Updates {
    pub(crate) patches: Vec<SubUpdate>,
    pub(crate) first_loads: Vec<SubUpdate>,
}

#[derive(Debug, Default)]
pub(crate) struct Registry {
    subs: HashMap<SubId, Sub>,
    opened: u64,
    app_forced: bool,
}

impl Registry {
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

    pub(crate) fn close(&mut self, id: &SubId) -> Option<SubKey> {
        self.subs.remove(id).map(|sub| sub.key)
    }

    pub(crate) fn close_all(&mut self) -> Vec<(SubId, SubKey)> {
        let mut all: Vec<(SubId, Sub)> = self.subs.drain().collect();
        all.sort_by_key(|(_, sub)| sub.opened);
        all.into_iter().map(|(id, sub)| (id, sub.key)).collect()
    }

    pub(crate) fn watches_thread(&self, thread: ThreadId) -> bool {
        self.subs
            .values()
            .any(|s| matches!(&s.key, SubKey::Conversation { thread: t, .. } if *t == thread))
    }

    pub(crate) fn has_composer(&self, draft: &DraftId) -> bool {
        self.subs.values().any(|s| is_composer_of(&s.key, draft))
    }

    pub(crate) fn is_open(&self, thread: ThreadId) -> bool {
        self.subs.values().any(|s| {
            matches!(&s.key, SubKey::Conversation { thread: t, open: true, .. } if *t == thread)
        })
    }

    pub(crate) fn composer_drafts(&self) -> Vec<DraftId> {
        let mut open: Vec<(&Sub, &DraftId)> = self
            .subs
            .values()
            .filter_map(|s| {
                if let SubKey::Composer { draft } = &s.key {
                    Some((s, draft))
                } else {
                    None
                }
            })
            .collect();
        open.sort_by_key(|(s, _)| s.opened);
        open.into_iter().map(|(_, d)| d.clone()).collect()
    }

    pub(crate) fn close_composers(&mut self, draft: &DraftId) {
        self.subs.retain(|_, s| !is_composer_of(&s.key, draft));
    }

    pub(crate) fn get(&self, id: &SubId) -> Option<&Sub> {
        self.subs.get(id)
    }

    pub(crate) fn get_mut(&mut self, id: &SubId) -> Option<&mut Sub> {
        self.subs.get_mut(id)
    }

    pub(crate) const fn force_app(&mut self) {
        self.app_forced = true;
    }

    pub(crate) fn set_searching(&mut self, id: &SubId, searching: bool) {
        if let Some(sub) = self.subs.get_mut(id)
            && sub.searching != searching
        {
            sub.searching = searching;
            sub.forced = true;
        }
    }

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

    pub(crate) fn updates(
        &mut self,
        reads: Reads<'_>,
        changes: &Changes,
        online: bool,
        now: Millis,
    ) -> Updates {
        let source = Source { reads, online, now };
        let app_forced = std::mem::take(&mut self.app_forced);
        let mut subs: Vec<(&SubId, &mut Sub)> = self.subs.iter_mut().collect();
        subs.sort_by_key(|(_, sub)| sub.opened);
        let mut out = Updates::default();
        for (id, sub) in subs {
            let forced = std::mem::take(&mut sub.forced) || (app_forced && sub.key == SubKey::App);
            if sub.first_sent {
                out.patches.extend(patch(source, id, sub, changes, forced));
            } else {
                out.first_loads.extend(first_load(source, id, sub));
            }
        }
        out
    }
}

fn patch(
    source: Source<'_>,
    id: &SubId,
    sub: &mut Sub,
    changes: &Changes,
    forced: bool,
) -> Option<SubUpdate> {
    match build::patch(source, id, sub, changes, forced) {
        Ok(update) => update,
        Err(e) => {
            tracing::warn!(sub = %id, error = %e, "couldn't build a patch; sending a first load next");
            sub.first_sent = false;
            None
        }
    }
}

fn first_load(source: Source<'_>, id: &SubId, sub: &mut Sub) -> Option<SubUpdate> {
    match build::first_load(source, id, sub) {
        Ok(update) => {
            sub.first_sent = true;
            Some(update)
        }
        Err(e) => {
            tracing::warn!(sub = %id, error = %e, "couldn't build a first load; trying again next commit");
            None
        }
    }
}

fn is_composer_of(key: &SubKey, draft: &DraftId) -> bool {
    matches!(key, SubKey::Composer { draft: d } if d == draft)
}
