use crate::{Addr, GmailMessageId, LabelSet, MessageId, Millis, SystemLabel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFacts {
    pub id: MessageId,
    pub gmail_id: GmailMessageId,
    pub date: Millis,
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub snippet: String,
    pub is_draft: bool,
    pub is_list: bool,
    pub has_attachments: bool,
    pub labels: LabelSet,
}

impl MessageFacts {
    #[must_use]
    pub fn has(&self, label: SystemLabel) -> bool {
        self.labels.has(label)
    }

    #[must_use]
    pub fn is_trashed_or_spam(&self) -> bool {
        self.has(SystemLabel::Trash) || self.has(SystemLabel::Spam)
    }

    #[must_use]
    pub fn is_live(&self) -> bool {
        !self.is_draft && !self.is_trashed_or_spam()
    }

    #[must_use]
    pub fn sender_display(&self) -> &str {
        self.from
            .name
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.from.email)
    }
}
