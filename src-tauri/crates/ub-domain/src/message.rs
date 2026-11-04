//! A stored message as the rules read it.

use crate::{Addr, GmailMessageId, LabelSet, MessageId, Millis, SystemLabel};

/// What the rules know about one stored message: its headers, flags, and own
/// Gmail label ids. No body; local search passes that separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFacts {
    /// The message's row.
    pub id: MessageId,
    /// Gmail's id, which ops send.
    pub gmail_id: GmailMessageId,
    /// Gmail's `internalDate`.
    pub date: Millis,
    /// The `From` mailbox.
    pub from: Addr,
    /// The `To` mailboxes.
    pub to: Vec<Addr>,
    /// The `Cc` mailboxes.
    pub cc: Vec<Addr>,
    /// The `Bcc` mailboxes (only on mail this account sent).
    pub bcc: Vec<Addr>,
    /// The `Subject`, decoded.
    pub subject: String,
    /// Gmail's snippet.
    pub snippet: String,
    /// A Gmail draft's message.
    pub is_draft: bool,
    /// Has a `List-Id` or `List-Unsubscribe` header.
    pub is_list: bool,
    /// Has at least one attachment that is not inline.
    pub has_attachments: bool,
    /// This message's own Gmail label ids.
    pub labels: LabelSet,
}

impl MessageFacts {
    /// Whether the message carries this system label.
    pub fn has(&self, label: SystemLabel) -> bool {
        self.labels.has(label)
    }

    /// In Trash or Spam.
    pub fn is_trashed_or_spam(&self) -> bool {
        self.has(SystemLabel::Trash) || self.has(SystemLabel::Spam)
    }

    /// Not a draft and not in Trash or Spam (SPEC Views). Only live messages
    /// put a thread in Inbox, Starred, Sent, and label views.
    pub fn is_live(&self) -> bool {
        !self.is_draft && !self.is_trashed_or_spam()
    }

    /// The sender as a list row shows it: the display name, or the address
    /// when there is none.
    pub fn sender_display(&self) -> &str {
        self.from
            .name
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.from.email)
    }
}
