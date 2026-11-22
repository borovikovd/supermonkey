use crate::{LabelId, SystemLabel, ThreadDigest, View, query};

impl View {
    #[must_use]
    pub fn lists(&self, thread: &ThreadDigest<'_>, snoozed: bool) -> bool {
        match self {
            Self::Inbox => thread.in_inbox() && !snoozed,
            Self::Done => !thread.in_inbox() && !snoozed && thread.has_live,
            Self::AllMail => thread.has_live,
            Self::Snoozed => snoozed,
            Self::Starred | Self::Sent | Self::Label { .. } | Self::Trash | Self::Spam => self
                .label()
                .is_some_and(|label| thread.labels.contains(label)),
            Self::Drafts => false,
        }
    }

    #[must_use]
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Starred => Some(SystemLabel::Starred.id()),
            Self::Sent => Some(SystemLabel::Sent.id()),
            Self::Trash => Some(SystemLabel::Trash.id()),
            Self::Spam => Some(SystemLabel::Spam.id()),
            Self::Label { label } => Some(label.as_str()),
            Self::Inbox | Self::Done | Self::AllMail | Self::Snoozed | Self::Drafts => None,
        }
    }

    #[must_use]
    pub fn gmail_listing(&self) -> Option<GmailListing> {
        match self {
            Self::Done => Some(GmailListing::Query("-in:inbox -in:draft".to_owned())),
            Self::AllMail => Some(GmailListing::Query("-in:draft".to_owned())),
            Self::Trash => Some(GmailListing::label(SystemLabel::Trash.label_id(), true)),
            Self::Spam => Some(GmailListing::label(SystemLabel::Spam.label_id(), true)),
            Self::Label { label } => Some(GmailListing::label(label.clone(), false)),
            Self::Inbox | Self::Snoozed | Self::Starred | Self::Sent | Self::Drafts => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmailListing {
    Query(String),
    Label {
        label: LabelId,
        include_spam_trash: bool,
    },
}

impl GmailListing {
    #[must_use]
    pub fn search(raw: &str) -> Option<Self> {
        let q = query::gmail_query(raw);
        (!q.trim().is_empty()).then_some(Self::Query(q))
    }

    const fn label(label: LabelId, include_spam_trash: bool) -> Self {
        Self::Label {
            label,
            include_spam_trash,
        }
    }
}
