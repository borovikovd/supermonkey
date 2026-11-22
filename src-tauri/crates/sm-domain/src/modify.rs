use serde::{Deserialize, Serialize};

use crate::{LabelSet, MessageFacts, MessageId, Millis, ModifyKind, SystemLabel, Thread, ThreadId};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModifyError {
    #[error("mark unread can't change other labels")]
    UnreadWithOtherLabels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelDelta {
    add: LabelSet,
    remove: LabelSet,
}

impl LabelDelta {
    pub fn new(add: LabelSet, mut remove: LabelSet) -> Result<Self, ModifyError> {
        for label in &add {
            remove.remove(label.as_str());
        }
        if add.has(SystemLabel::Unread) && (add.len() > 1 || !remove.is_empty()) {
            return Err(ModifyError::UnreadWithOtherLabels);
        }
        Ok(Self { add, remove })
    }

    #[must_use]
    pub fn preset(kind: ModifyKind) -> Option<Self> {
        use SystemLabel::{Inbox, Spam, Starred, Trash, Unread};
        let (add, remove): (&[SystemLabel], &[SystemLabel]) = match kind {
            ModifyKind::Done => (&[], &[Inbox, Unread]),
            ModifyKind::ReturnToInbox => (&[Inbox], &[]),
            ModifyKind::Snooze => (&[], &[Inbox]),
            ModifyKind::Trash => (&[Trash], &[Inbox, Spam]),
            ModifyKind::Spam => (&[Spam], &[Inbox, Trash]),
            ModifyKind::Star => (&[Starred], &[]),
            ModifyKind::Unstar => (&[], &[Starred]),
            ModifyKind::Read => (&[], &[Unread]),
            ModifyKind::Unread => (&[Unread], &[]),
            ModifyKind::Label | ModifyKind::RemoveLabel => return None,
        };
        Some(Self {
            add: add.iter().copied().collect(),
            remove: remove.iter().copied().collect(),
        })
    }

    #[must_use]
    pub const fn add(&self) -> &LabelSet {
        &self.add
    }

    #[must_use]
    pub const fn remove(&self) -> &LabelSet {
        &self.remove
    }

    #[must_use]
    pub fn into_parts(self) -> (LabelSet, LabelSet) {
        (self.add, self.remove)
    }

    #[must_use]
    fn marks_unread(&self) -> bool {
        self.add.has(SystemLabel::Unread)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageChange {
    pub message: MessageId,
    pub added: LabelSet,
    pub removed: LabelSet,
}

impl MessageChange {
    #[must_use]
    pub fn apply(&self, mut labels: LabelSet) -> LabelSet {
        for gone in &self.removed {
            labels.remove(gone.as_str());
        }
        labels.extend(self.added.iter().cloned());
        labels
    }

    #[must_use]
    pub fn reverted(&self, current: &LabelSet) -> Option<Self> {
        let added = self.removed.without(current);
        let removed = self.added.and(current);
        (!added.is_empty() || !removed.is_empty()).then_some(Self {
            message: self.message,
            added,
            removed,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadChange {
    pub thread: ThreadId,
    pub changes: Vec<MessageChange>,
    pub snooze_before: Option<Millis>,
    pub snooze_after: Option<Millis>,
}

impl ThreadChange {
    #[must_use]
    pub fn restores_snooze(&self, now: Option<Millis>) -> bool {
        self.snooze_after != self.snooze_before && now == self.snooze_after
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnoozeEdit {
    Set(Millis),
    Clear,
}

#[must_use]
pub const fn snooze_after(
    edit: Option<SnoozeEdit>,
    kind: ModifyKind,
    before: Option<Millis>,
) -> Option<Millis> {
    match edit {
        Some(SnoozeEdit::Set(at)) => Some(at),
        Some(SnoozeEdit::Clear) => None,
        None if matches!(
            kind,
            ModifyKind::Done | ModifyKind::Trash | ModifyKind::Spam
        ) =>
        {
            None
        }
        None => before,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadModify<'a> {
    pub op_messages: Vec<&'a MessageFacts>,
    pub changes: Vec<MessageChange>,
}

#[must_use]
pub fn plan_modify<'a>(thread: &'a Thread, delta: &LabelDelta) -> ThreadModify<'a> {
    let latest_live = thread.latest_live().map(|m| m.id);
    let targeted =
        |m: &MessageFacts| !delta.marks_unread() || latest_live.is_none_or(|latest| latest == m.id);
    let mut plan = ThreadModify {
        op_messages: Vec::new(),
        changes: Vec::new(),
    };
    for m in thread.non_drafts().filter(|m| targeted(m)) {
        plan.op_messages.push(m);
        let added = delta.add.without(&m.labels);
        let removed = delta.remove.and(&m.labels);
        if !added.is_empty() || !removed.is_empty() {
            plan.changes.push(MessageChange {
                message: m.id,
                added,
                removed,
            });
        }
    }
    plan
}
