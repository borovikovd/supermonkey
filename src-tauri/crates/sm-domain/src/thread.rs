//! A thread, derived from its messages (SPEC Views): the latest message, the
//! labels that put it in views, and the values its list row shows.

use crate::{Identity, LabelSet, MessageFacts, MessageId, Millis, SystemLabel};

/// A thread's stored messages, oldest first (by date, then row id).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Thread {
    messages: Vec<MessageFacts>,
}

impl Thread {
    /// The thread holding these messages, in any order.
    pub fn new(mut messages: Vec<MessageFacts>) -> Self {
        messages.sort_by_key(|m| (m.date, m.id));
        Self { messages }
    }

    /// Every message, drafts included, oldest first.
    pub fn messages(&self) -> &[MessageFacts] {
        &self.messages
    }

    /// The message with this row id.
    pub fn message(&self, id: MessageId) -> Option<&MessageFacts> {
        self.messages.iter().find(|m| m.id == id)
    }

    /// The messages that are not drafts, oldest first.
    pub fn non_drafts(&self) -> impl DoubleEndedIterator<Item = &MessageFacts> + '_ {
        self.messages.iter().filter(|m| !m.is_draft)
    }

    /// The newest live message (not a draft, not in Trash or Spam).
    pub fn latest_live(&self) -> Option<&MessageFacts> {
        self.messages.iter().rev().find(|m| m.is_live())
    }

    /// The thread's latest message (SPEC Views): the newest live message,
    /// else the newest non-draft, else the newest draft. `None` only for a
    /// thread with no messages.
    pub fn latest(&self) -> Option<&MessageFacts> {
        self.latest_live()
            .or_else(|| self.non_drafts().next_back())
            .or_else(|| self.messages.last())
    }

    /// The message split rules and pins read (SPEC Where a conversation
    /// lands): the newest live message not sent from this account, or the
    /// latest message when every one is from this account. So your own
    /// reply never moves a thread.
    pub fn latest_from_someone_else(&self, me: &Identity) -> Option<&MessageFacts> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.is_live() && !me.is_mine(&m.from.email))
            .or_else(|| self.latest())
    }

    /// Whether any message is live. Done, All Mail, and Search skip threads
    /// with none.
    pub fn has_live(&self) -> bool {
        self.messages.iter().any(MessageFacts::is_live)
    }

    /// The labels the thread is in (SPEC Views): every label of a live
    /// message, plus `TRASH` or `SPAM` when any message has it. `DRAFT` never.
    pub fn labels(&self) -> LabelSet {
        let mut set = LabelSet::new();
        for m in &self.messages {
            if m.is_live() {
                set.extend(
                    m.labels
                        .iter()
                        .filter(|l| l.as_str() != SystemLabel::Draft.id())
                        .cloned(),
                );
            }
            for label in [SystemLabel::Trash, SystemLabel::Spam] {
                if m.has(label) {
                    set.insert(label.label_id());
                }
            }
        }
        set
    }

    /// The thread's derived row, or `None` when it has no messages left and
    /// its row should go.
    pub fn digest(&self) -> Option<ThreadDigest> {
        let latest = self.latest()?;
        let live = self.messages.iter().filter(|m| m.is_live()).count();
        let non_draft = self.non_drafts().count();
        let shown = if live > 0 { live } else { non_draft.max(1) };
        let unread = if live > 0 {
            self.messages
                .iter()
                .any(|m| m.is_live() && m.has(SystemLabel::Unread))
        } else {
            self.non_drafts().any(|m| m.has(SystemLabel::Unread))
        };
        let subject = self
            .non_drafts()
            .find(|m| !m.subject.is_empty())
            .unwrap_or(latest)
            .subject
            .clone();
        Some(ThreadDigest {
            subject,
            snippet: latest.snippet.clone(),
            sender: latest.sender_display().to_owned(),
            sender_email: latest.from.email.clone(),
            sort_time: latest.date,
            message_count: u32::try_from(shown).unwrap_or(u32::MAX),
            unread,
            has_attachments: self.non_drafts().any(|m| m.has_attachments),
            has_live: live > 0,
            labels: self.labels(),
        })
    }

    /// The snooze rule after a history batch (SPEC Snooze): a snoozed thread
    /// that has gained `INBOX` from outside this app loses its snooze. True
    /// when a live message has `INBOX`, skipping messages for which
    /// `has_pending_op` is true, since their labels are this app's own
    /// change still on its way to Gmail.
    pub fn gained_inbox(&self, has_pending_op: impl Fn(&MessageFacts) -> bool) -> bool {
        self.messages
            .iter()
            .any(|m| m.is_live() && m.has(SystemLabel::Inbox) && !has_pending_op(m))
    }
}

/// A thread's list row values, derived from its messages by [`Thread::digest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDigest {
    /// The first non-draft message's subject that is not empty, else the
    /// latest message's.
    pub subject: String,
    /// The latest message's snippet.
    pub snippet: String,
    /// The latest message's sender name, or address when it has no name.
    pub sender: String,
    /// The latest message's sender address.
    pub sender_email: String,
    /// The latest message's date; lists sort by it.
    pub sort_time: Millis,
    /// Live messages, or when there are none the non-drafts, at least 1.
    pub message_count: u32,
    /// A live message is unread, or with none live, a non-draft one is.
    pub unread: bool,
    /// A non-draft message has an attachment.
    pub has_attachments: bool,
    /// A message is live.
    pub has_live: bool,
    /// The thread's labels, by [`Thread::labels`].
    pub labels: LabelSet,
}

impl ThreadDigest {
    /// In the Inbox (snoozed or not).
    pub fn in_inbox(&self) -> bool {
        self.labels.has(SystemLabel::Inbox)
    }

    /// In Trash.
    pub fn in_trash(&self) -> bool {
        self.labels.has(SystemLabel::Trash)
    }

    /// In Spam.
    pub fn in_spam(&self) -> bool {
        self.labels.has(SystemLabel::Spam)
    }

    /// Starred.
    pub fn starred(&self) -> bool {
        self.labels.has(SystemLabel::Starred)
    }
}
