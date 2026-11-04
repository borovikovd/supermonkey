//! A poll's `history.list` records, reduced to what changed (SPEC Sync).

use std::collections::{BTreeMap, BTreeSet};

use ub_domain::{GmailMessageId, GmailThreadId, LabelSet, SystemLabel};
use ub_gmail::{HistoryId, HistoryRecord, Message};

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
                batch.touch(record.id, m.thread_id.clone());
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

#[cfg(test)]
mod tests {
    use ub_domain::LabelId;
    use ub_gmail::{HistoryLabelChange, HistoryMessage};

    use super::*;

    fn message(id: &str, thread: &str, labels: &[&str]) -> Message {
        Message {
            id: GmailMessageId::from(id),
            thread_id: GmailThreadId::from(thread),
            label_ids: labels.iter().map(|l| LabelId::from(*l)).collect(),
            snippet: String::new(),
            history_id: None,
            internal_date: None,
            payload: None,
            raw: None,
        }
    }

    fn record(id: u64) -> HistoryRecord {
        HistoryRecord {
            id: HistoryId(id),
            messages: Vec::new(),
            messages_added: Vec::new(),
            messages_deleted: Vec::new(),
            labels_added: Vec::new(),
            labels_removed: Vec::new(),
        }
    }

    fn labels(batch: &HistoryBatch, id: &str) -> Vec<String> {
        batch.labels[&GmailMessageId::from(id)]
            .1
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn the_last_record_naming_a_message_sets_its_labels() {
        let mut first = record(10);
        first.messages_added.push(HistoryMessage {
            message: message("m1", "t1", &["INBOX", "UNREAD"]),
        });
        let mut second = record(11);
        second.labels_removed.push(HistoryLabelChange {
            message: message("m1", "t1", &["INBOX"]),
            label_ids: vec![LabelId::from("UNREAD")],
        });
        let batch = HistoryBatch::new(vec![first, second]);
        assert_eq!(labels(&batch, "m1"), ["INBOX"]);
        assert!(batch.added.contains(&GmailMessageId::from("m1")));
        assert_eq!(batch.newest[&GmailThreadId::from("t1")], HistoryId(11));
        assert!(!batch.drafts_changed);
    }

    #[test]
    fn deleted_messages_need_no_read_and_mark_drafts_changed() {
        let mut first = record(5);
        first.messages_added.push(HistoryMessage {
            message: message("m1", "t1", &["INBOX"]),
        });
        first.messages_added.push(HistoryMessage {
            message: message("m2", "t2", &["INBOX"]),
        });
        let mut second = record(4);
        second.messages_deleted.push(HistoryMessage {
            message: message("m2", "t2", &[]),
        });
        let batch = HistoryBatch::new(vec![first, second]);
        let to_read = batch.threads_to_read(|id| id.as_str() == "known");
        assert_eq!(
            to_read.into_iter().collect::<Vec<_>>(),
            [GmailThreadId::from("t1")]
        );
        assert!(batch.drafts_changed, "a delete may be an old draft version");
        assert_eq!(
            batch.newest[&GmailThreadId::from("t2")],
            HistoryId(5),
            "newest wins whatever the order"
        );
    }

    #[test]
    fn a_draft_added_marks_drafts_changed_and_known_messages_need_no_read() {
        let mut r = record(1);
        r.messages_added.push(HistoryMessage {
            message: message("d1", "t1", &["DRAFT"]),
        });
        let batch = HistoryBatch::new(vec![r]);
        assert!(batch.drafts_changed);
        assert!(batch.threads_to_read(|_| true).is_empty());
    }
}
