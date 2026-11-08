//! Builders shared by the unit tests.

use crate::{Addr, GmailMessageId, LabelSet, MessageFacts, MessageId, Millis};

/// A non-draft, non-list message `id` from `from`, dated `id` seconds in,
/// with `labels` and nothing else.
pub(crate) fn message(id: i64, from: Addr, labels: &[&str]) -> MessageFacts {
    MessageFacts {
        id: MessageId(id),
        gmail_id: GmailMessageId(format!("g{id}")),
        date: Millis(id * 1000),
        from,
        to: Vec::new(),
        cc: Vec::new(),
        bcc: Vec::new(),
        subject: String::new(),
        snippet: String::new(),
        is_draft: false,
        is_list: false,
        has_attachments: false,
        labels: labels.iter().copied().collect::<LabelSet>(),
    }
}

/// `labels` as a set.
pub(crate) fn set(labels: &[&str]) -> LabelSet {
    labels.iter().copied().collect()
}
