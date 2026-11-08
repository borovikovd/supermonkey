//! A poll's `history.list` records, reduced to what changed (SPEC Sync).

use std::collections::{BTreeMap, BTreeSet};

use sm_domain::{GmailMessageId, GmailThreadId, LabelSet, SystemLabel};
use sm_gmail::{HistoryId, HistoryRecord, Message};

/// What a run of history records says, net.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HistoryBatch {
    /// Each message's thread and its label set in the last record that named
    /// it: records carry the whole set, so the last one wins.
    pub(crate) labels: BTreeMap<GmailMessageId, (GmailThreadId, LabelSet)>,
    /// Messages added.
    pub(crate) added: BTreeSet<GmailMessageId>,
    /// Messages deleted.
    pub(crate) deleted: BTreeSet<GmailMessageId>,
    /// Each thread's newest record.
    pub(crate) newest: BTreeMap<GmailThreadId, HistoryId>,
    /// A draft was added, or anything deleted (an old draft version):
    /// drafts need reconciling (SPEC Drafts).
    pub(crate) drafts_changed: bool,
}

impl HistoryBatch {
    /// Reduces records, oldest first.
    pub(crate) fn new(records: Vec<HistoryRecord>) -> Self {
        let mut batch = Self::default();
        for record in records {
            for added in record.messages_added {
                let m = added.message;
                batch.drafts_changed |= m
                    .label_ids
                    .iter()
                    .any(|l| l.as_str() == SystemLabel::Draft.id());
                batch.added.insert(m.id.clone());
                batch.note(record.id, m);
            }
            for change in record.labels_added.into_iter().chain(record.labels_removed) {
                batch.note(record.id, change.message);
            }
            for deleted in record.messages_deleted {
                let m = deleted.message;
                batch.touch(record.id, m.thread_id);
                batch.deleted.insert(m.id);
            }
        }
        batch.drafts_changed |= !batch.deleted.is_empty();
        batch
    }

    fn note(&mut self, at: HistoryId, m: Message) {
        self.touch(at, m.thread_id.clone());
        let labels = m.label_ids.into_iter().collect();
        self.labels.insert(m.id, (m.thread_id, labels));
    }

    fn touch(&mut self, at: HistoryId, thread: GmailThreadId) {
        let newest = self.newest.entry(thread).or_insert(at);
        *newest = (*newest).max(at);
    }

    /// Threads that have a message the batch names and the app has not
    /// stored, by `is_known`: they need a metadata read. Deleted messages
    /// need none.
    pub(crate) fn threads_to_read(
        &self,
        is_known: impl Fn(&GmailMessageId) -> bool,
    ) -> BTreeSet<GmailThreadId> {
        self.labels
            .iter()
            .filter(|(id, _)| !self.deleted.contains(*id) && !is_known(id))
            .map(|(_, (thread, _))| thread.clone())
            .collect()
    }
}
