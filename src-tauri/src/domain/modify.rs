use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    GmailMessageId, LabelId, LabelSet, MessageId, Millis, ModifyKind, SystemLabel, Thread, ThreadId,
};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModifyError {
    #[error("UNREAD can't be mixed with other label changes")]
    UnreadMixed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelDelta {
    add: Vec<LabelId>,
    remove: Vec<LabelId>,
}

impl LabelDelta {
    pub fn new(add: Vec<LabelId>, remove: Vec<LabelId>) -> Result<Self, ModifyError> {
        let adds_unread = add.iter().any(|l| l.0 == SystemLabel::Unread.id());
        let has_other = add
            .iter()
            .chain(&remove)
            .any(|l| l.0 != SystemLabel::Unread.id());
        if adds_unread && has_other {
            return Err(ModifyError::UnreadMixed);
        }
        Ok(Self { add, remove })
    }

    #[must_use]
    pub fn add(&self) -> &[LabelId] {
        &self.add
    }

    #[must_use]
    pub fn remove(&self) -> &[LabelId] {
        &self.remove
    }

    #[must_use]
    pub fn adds_unread_only(&self) -> bool {
        self.remove.is_empty()
            && !self.add.is_empty()
            && self.add.iter().all(|l| l.0 == SystemLabel::Unread.id())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageChange {
    pub message: MessageId,
    pub gmail_id: GmailMessageId,
    pub added: Vec<LabelId>,
    pub removed: Vec<LabelId>,
}

impl MessageChange {
    #[must_use]
    pub fn apply(&self, current: &LabelSet) -> LabelSet {
        current.without(&self.removed).and(&self.added)
    }

    #[must_use]
    pub fn reverted(&self, current: &LabelSet) -> Option<Self> {
        let added: Vec<LabelId> = self
            .removed
            .iter()
            .filter(|l| !current.contains(l.as_str()))
            .cloned()
            .collect();
        let removed: Vec<LabelId> = self
            .added
            .iter()
            .filter(|l| current.contains(l.as_str()))
            .cloned()
            .collect();
        if added.is_empty() && removed.is_empty() {
            return None;
        }
        Some(Self {
            message: self.message,
            gmail_id: self.gmail_id.clone(),
            added,
            removed,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnoozeEdit {
    Set(Millis),
    Clear,
}

#[must_use]
pub const fn snooze_after(
    kind: ModifyKind,
    current: Option<Millis>,
    edit: Option<SnoozeEdit>,
) -> Option<Millis> {
    match edit {
        Some(SnoozeEdit::Set(at)) => Some(at),
        Some(SnoozeEdit::Clear) => None,
        None => match kind {
            ModifyKind::Done | ModifyKind::ReturnToInbox | ModifyKind::Trash | ModifyKind::Spam => {
                None
            }
            ModifyKind::Snooze
            | ModifyKind::Star
            | ModifyKind::Unstar
            | ModifyKind::Read
            | ModifyKind::Unread
            | ModifyKind::Label
            | ModifyKind::RemoveLabel => current,
        },
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ThreadModify<'a> {
    pub thread_id: ThreadId,
    pub thread: &'a Thread,
    pub kind: ModifyKind,
    pub snooze_before: Option<Millis>,
    pub snooze_edit: Option<SnoozeEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadChange {
    pub thread: ThreadId,
    pub messages: Vec<MessageChange>,
    pub snooze_before: Option<Millis>,
    pub snooze_after: Option<Millis>,
}

impl ThreadChange {
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.messages.is_empty() && self.snooze_before == self.snooze_after
    }

    #[must_use]
    pub fn restores_snooze(&self) -> bool {
        self.snooze_before.is_some() && self.snooze_before != self.snooze_after
    }

    #[must_use]
    pub fn snooze_undone(&self, current: Option<Millis>) -> Option<Millis> {
        if self.snooze_before != self.snooze_after && current == self.snooze_after {
            self.snooze_before
        } else {
            current
        }
    }
}

#[must_use]
pub fn plan_modify(input: ThreadModify<'_>, delta: &LabelDelta) -> ThreadChange {
    let snooze_next = snooze_after(input.kind, input.snooze_before, input.snooze_edit);
    let mut messages = Vec::new();

    if delta.adds_unread_only() {
        if let Some(target) = input.thread.latest_live() {
            push_change(&mut messages, target, delta);
        }
    } else {
        for msg in input.thread.messages().iter().filter(|m| !m.is_draft) {
            push_change(&mut messages, msg, delta);
        }
    }

    ThreadChange {
        thread: input.thread_id,
        messages,
        snooze_before: input.snooze_before,
        snooze_after: snooze_next,
    }
}

fn push_change(out: &mut Vec<MessageChange>, msg: &super::MessageFacts, delta: &LabelDelta) {
    let added: Vec<LabelId> = delta
        .add()
        .iter()
        .filter(|l| !msg.labels.contains(l.as_str()))
        .cloned()
        .collect();
    let removed: Vec<LabelId> = delta
        .remove()
        .iter()
        .filter(|l| msg.labels.contains(l.as_str()))
        .cloned()
        .collect();
    if !added.is_empty() || !removed.is_empty() {
        out.push(MessageChange {
            message: msg.id,
            gmail_id: msg.gmail_id.clone(),
            added,
            removed,
        });
    }
}
