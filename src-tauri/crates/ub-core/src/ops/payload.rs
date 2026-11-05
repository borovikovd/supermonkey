//! The payloads of queued ops, stored as JSON in the store's op queue.

use serde::{Deserialize, Serialize};
use ub_domain::{GmailMessageId, LabelId, LabelSet, ThreadChange};

/// A `messages.batchModify`. Its label ids are local ids: the runner maps
/// them to Gmail's when it runs, so a label created offline has its Gmail
/// id by then (SPEC Offline).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModifyOp {
    /// The messages, as they were when the action ran: a replay or retry
    /// never touches mail that arrived later (SPEC Archive, trash, star,
    /// read).
    pub(crate) messages: Vec<GmailMessageId>,
    /// Labels to add.
    pub(crate) add: LabelSet,
    /// Labels to remove.
    pub(crate) remove: LabelSet,
    /// What the action changed here, rolled back if Gmail rejects it.
    pub(crate) threads: Vec<ThreadChange>,
}

impl ModifyOp {
    /// Whether it names `message`.
    pub(crate) fn names(&self, message: &GmailMessageId) -> bool {
        self.messages.contains(message)
    }

    /// Puts its change on top of `labels`.
    pub(crate) fn apply_to(&self, labels: &mut LabelSet) {
        *labels = labels.without(&self.remove);
        labels.extend(self.add.iter().cloned());
    }

    /// Whether it adds or removes `label`.
    pub(crate) fn uses(&self, label: &LabelId) -> bool {
        self.add.contains(label.as_str()) || self.remove.contains(label.as_str())
    }

    /// Renames `from` to `to` everywhere in it. Returns whether it named
    /// `from`.
    pub(crate) fn relabel(&mut self, from: &LabelId, to: &LabelId) -> bool {
        let mut named = relabel_set(&mut self.add, from, to);
        named |= relabel_set(&mut self.remove, from, to);
        named | relabel_changes(&mut self.threads, from, to)
    }
}

/// A `labels.create` of a label made here, under its local id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LabelCreateOp {
    /// The UI's id for it, until Gmail gives it one.
    pub(crate) label: LabelId,
    /// Its name.
    pub(crate) name: String,
}

/// Renames `from` to `to` in thread changes. Returns whether any named it.
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
