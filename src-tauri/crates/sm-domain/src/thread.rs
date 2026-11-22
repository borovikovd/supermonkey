use crate::{Identity, LabelSet, MessageFacts, Millis, SystemLabel};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Thread {
    messages: Vec<MessageFacts>,
}

impl Thread {
    #[must_use]
    pub fn new(mut messages: Vec<MessageFacts>) -> Self {
        messages.sort_by_key(|m| (m.date, m.id));
        Self { messages }
    }

    #[must_use]
    pub fn messages(&self) -> &[MessageFacts] {
        &self.messages
    }

    pub fn non_drafts(&self) -> impl DoubleEndedIterator<Item = &MessageFacts> + '_ {
        self.messages.iter().filter(|m| !m.is_draft)
    }

    #[must_use]
    pub fn latest_live(&self) -> Option<&MessageFacts> {
        self.messages.iter().rev().find(|m| m.is_live())
    }

    #[must_use]
    pub fn latest(&self) -> Option<&MessageFacts> {
        self.latest_live()
            .or_else(|| self.non_drafts().next_back())
            .or_else(|| self.messages.last())
    }

    #[must_use]
    pub fn latest_from_someone_else(&self, me: &Identity) -> Option<&MessageFacts> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.is_live() && !me.is_mine(&m.from.email))
            .or_else(|| self.latest())
    }

    #[must_use]
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

    #[must_use]
    pub fn digest(&self) -> Option<ThreadDigest<'_>> {
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
            .as_str();
        Some(ThreadDigest {
            subject,
            snippet: &latest.snippet,
            sender: latest.sender_display(),
            sender_email: &latest.from.email,
            sort_time: latest.date,
            message_count: u32::try_from(shown).unwrap_or(u32::MAX),
            unread,
            has_attachments: self.non_drafts().any(|m| m.has_attachments),
            has_live: live > 0,
            labels: self.labels(),
        })
    }

    pub fn gained_inbox(&self, has_pending_op: impl Fn(&MessageFacts) -> bool) -> bool {
        self.messages
            .iter()
            .any(|m| m.is_live() && m.has(SystemLabel::Inbox) && !has_pending_op(m))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadDigest<'a> {
    pub subject: &'a str,
    pub snippet: &'a str,
    pub sender: &'a str,
    pub sender_email: &'a str,
    pub sort_time: Millis,
    pub message_count: u32,
    pub unread: bool,
    pub has_attachments: bool,
    pub has_live: bool,
    pub labels: LabelSet,
}

impl ThreadDigest<'_> {
    #[must_use]
    pub fn in_inbox(&self) -> bool {
        self.labels.has(SystemLabel::Inbox)
    }

    #[must_use]
    pub fn in_trash(&self) -> bool {
        self.labels.has(SystemLabel::Trash)
    }

    #[must_use]
    pub fn in_spam(&self) -> bool {
        self.labels.has(SystemLabel::Spam)
    }
}
