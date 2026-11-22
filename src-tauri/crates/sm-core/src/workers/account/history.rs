use std::collections::{BTreeMap, BTreeSet};

use sm_domain::{GmailMessageId, GmailThreadId, LabelSet, SystemLabel};
use sm_gmail::{HistoryId, HistoryRecord, Message};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HistoryBatch {
    pub(crate) labels: BTreeMap<GmailMessageId, (GmailThreadId, LabelSet)>,
    pub(crate) added: BTreeSet<GmailMessageId>,
    pub(crate) deleted: BTreeSet<GmailMessageId>,
    pub(crate) newest: BTreeMap<GmailThreadId, HistoryId>,
    pub(crate) drafts_changed: bool,
}

impl HistoryBatch {
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
