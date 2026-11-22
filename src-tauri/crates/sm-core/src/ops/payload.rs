use serde::{Deserialize, Serialize};
use sm_domain::{GmailMessageId, LabelId, LabelSet, SystemLabel, ThreadChange};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModifyOp {
    pub(crate) messages: Vec<GmailMessageId>,
    pub(crate) add: LabelSet,
    pub(crate) remove: LabelSet,
    pub(crate) threads: Vec<ThreadChange>,
}

impl ModifyOp {
    pub(crate) fn trash(message: GmailMessageId) -> Self {
        Self {
            messages: vec![message],
            add: std::iter::once(SystemLabel::Trash).collect(),
            remove: [SystemLabel::Inbox, SystemLabel::Spam]
                .into_iter()
                .collect(),
            threads: Vec::new(),
        }
    }

    pub(crate) fn names(&self, message: &GmailMessageId) -> bool {
        self.messages.contains(message)
    }

    pub(crate) fn apply_to(&self, labels: &mut LabelSet) {
        *labels = labels.without(&self.remove);
        labels.extend(self.add.iter().cloned());
    }

    pub(crate) fn uses(&self, label: &LabelId) -> bool {
        self.add.contains(label.as_str()) || self.remove.contains(label.as_str())
    }

    pub(crate) fn relabel(&mut self, from: &LabelId, to: &LabelId) -> bool {
        let mut named = relabel_set(&mut self.add, from, to);
        named |= relabel_set(&mut self.remove, from, to);
        named | relabel_changes(&mut self.threads, from, to)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LabelCreateOp {
    pub(crate) label: LabelId,
    pub(crate) name: String,
}

pub(crate) fn relabel_changes(threads: &mut [ThreadChange], from: &LabelId, to: &LabelId) -> bool {
    let mut named = false;
    for change in threads.iter_mut().flat_map(|t| t.changes.iter_mut()) {
        named |= relabel_set(&mut change.added, from, to);
        named |= relabel_set(&mut change.removed, from, to);
    }
    named
}

fn relabel_set(set: &mut LabelSet, from: &LabelId, to: &LabelId) -> bool {
    let named = set.remove(from.as_str());
    if named {
        set.insert(to.clone());
    }
    named
}
