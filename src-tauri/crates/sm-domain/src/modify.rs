//! Mail actions as label changes (SPEC Archive, trash, star, read; Snooze;
//! Undo): which messages an action touches, what each one gains and loses,
//! and how undo and rollback revert exactly that.

use serde::{Deserialize, Serialize};

use crate::{LabelSet, MessageFacts, MessageId, Millis, ModifyKind, SystemLabel, Thread, ThreadId};

/// Why a [`LabelDelta`] is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModifyError {
    /// Mark unread changes the latest message only, so it can't share an op
    /// with labels meant for every message.
    #[error("mark unread can't change other labels")]
    UnreadWithOtherLabels,
}

/// The labels an action adds and removes. A label in both is only added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelDelta {
    add: LabelSet,
    remove: LabelSet,
}

impl LabelDelta {
    /// A delta, checked: adding `UNREAD` must be its only change.
    pub fn new(add: LabelSet, mut remove: LabelSet) -> Result<Self, ModifyError> {
        for label in &add {
            remove.remove(label.as_str());
        }
        if add.has(SystemLabel::Unread) && (add.len() > 1 || !remove.is_empty()) {
            return Err(ModifyError::UnreadWithOtherLabels);
        }
        Ok(Self { add, remove })
    }

    /// The fixed delta of an action kind, as the UI sends it; `None` for the
    /// label actions, which name their label. Return to inbox from Trash or
    /// Spam also removes `TRASH` or `SPAM`; that is the caller's to add.
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

    /// The labels added.
    pub fn add(&self) -> &LabelSet {
        &self.add
    }

    /// The labels removed.
    pub fn remove(&self) -> &LabelSet {
        &self.remove
    }

    /// The labels added and the labels removed, taken.
    pub fn into_parts(self) -> (LabelSet, LabelSet) {
        (self.add, self.remove)
    }

    /// Mark unread: adds `UNREAD` to the latest live message only, as Gmail does.
    pub fn marks_unread(&self) -> bool {
        self.add.has(SystemLabel::Unread)
    }
}

/// What an action did to one message's labels: only what really changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageChange {
    /// The message.
    pub message: MessageId,
    /// Labels it gained.
    pub added: LabelSet,
    /// Labels it lost.
    pub removed: LabelSet,
}

impl MessageChange {
    /// `labels` with this change applied.
    pub fn apply(&self, labels: &LabelSet) -> LabelSet {
        let mut out = labels.without(&self.removed);
        out.extend(self.added.iter().cloned());
        out
    }

    /// The change that undoes this one on a message whose labels are now
    /// `current`: remove what was added and is still there, add back what
    /// was removed and is still missing. Later changes made elsewhere stay.
    /// `None` when nothing is left to revert.
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

/// What an action did to one thread: its messages' label changes and its
/// snooze time before and after. Undo entries and op rollbacks store these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadChange {
    /// The thread.
    pub thread: ThreadId,
    /// The messages that changed.
    pub changes: Vec<MessageChange>,
    /// `snooze_until` before the action.
    pub snooze_before: Option<Millis>,
    /// `snooze_until` after it.
    pub snooze_after: Option<Millis>,
}

impl ThreadChange {
    /// Whether reverting puts `snooze_before` back: the action changed the
    /// snooze and nothing has changed it since (`now` is still its value).
    pub fn restores_snooze(&self, now: Option<Millis>) -> bool {
        self.snooze_after != self.snooze_before && now == self.snooze_after
    }
}

/// A change to a thread's snooze time that an action carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnoozeEdit {
    /// Snooze until this time.
    Set(Millis),
    /// Wake now.
    Clear,
}

/// A thread's `snooze_until` after an action: what the action sets, else
/// cleared by archive, trash, and spam, else unchanged.
pub fn snooze_after(
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

/// An action's effect on one thread, as [`plan_modify`] works it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadModify<'a> {
    /// The messages the Gmail `batchModify` names. Every non-draft message
    /// at the time of the action (so a replay never touches later mail),
    /// or for mark unread only the latest live one.
    pub op_messages: Vec<&'a MessageFacts>,
    /// The messages whose labels change, and how.
    pub changes: Vec<MessageChange>,
}

/// What an action with `delta` does to a thread. Drafts are never labeled:
/// Gmail does not label them, and their ids change on every update.
pub fn plan_modify<'a>(thread: &'a Thread, delta: &LabelDelta) -> ThreadModify<'a> {
    let latest_live = thread.latest_live().map(|m| m.id);
    // With no live message, mark unread falls back to every message.
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
