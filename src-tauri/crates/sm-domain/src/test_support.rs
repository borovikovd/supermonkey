use crate::{Addr, GmailMessageId, LabelSet, MessageFacts, MessageId, Millis};

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

pub(crate) fn set(labels: &[&str]) -> LabelSet {
    labels.iter().copied().collect()
}
