//! Which threads each view lists, and what Gmail listing backs it (SPEC Views).

use crate::{LabelId, SystemLabel, ThreadDigest, View, query};

impl View {
    /// Whether the view lists a thread with this digest. `snoozed` is whether
    /// the thread has `snooze_until` set, due or not. Drafts lists drafts,
    /// never threads.
    pub fn lists(&self, thread: &ThreadDigest, snoozed: bool) -> bool {
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

    /// The thread label that alone decides membership: Starred, Sent, Trash,
    /// Spam, and label views. `None` for the others.
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

    /// Whether the list sorts by snooze time, soonest first, instead of by
    /// the latest message, newest first.
    pub fn sorts_by_snooze(&self) -> bool {
        matches!(self, Self::Snoozed)
    }

    /// The Gmail `threads.list` the view also runs while online, since most
    /// archived, labeled, trashed, and spam mail is older than the first sync.
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

/// A Gmail `threads.list` request a list merges into its local results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GmailListing {
    /// A `q=` search, as Gmail should get it.
    Query(String),
    /// Threads with a label (by local id; the caller maps it to Gmail's).
    Label {
        /// The label's local id.
        label: LabelId,
        /// Set `includeSpamTrash=true`.
        include_spam_trash: bool,
    },
}

impl GmailListing {
    /// The listing for a search box query, or `None` when it is blank. Gmail
    /// gets the query without `is:list`, which it does not know.
    pub fn search(raw: &str) -> Option<Self> {
        (!raw.trim().is_empty()).then(|| Self::Query(query::gmail_query(raw)))
    }

    fn label(label: LabelId, include_spam_trash: bool) -> Self {
        Self::Label {
            label,
            include_spam_trash,
        }
    }
}
