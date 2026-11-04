//! Gmail label ids: the system ones by name, and sets of them.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::LabelId;

/// Gmail's prefix for category labels (`CATEGORY_SOCIAL`, ...).
const CATEGORY_PREFIX: &str = "CATEGORY_";

/// A Gmail system or category label this app knows by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SystemLabel {
    /// `INBOX`.
    Inbox,
    /// `SENT`.
    Sent,
    /// `DRAFT`.
    Draft,
    /// `STARRED`.
    Starred,
    /// `UNREAD`.
    Unread,
    /// `IMPORTANT`, Gmail's own importance marker (not the Important split).
    Important,
    /// `TRASH`.
    Trash,
    /// `SPAM`.
    Spam,
    /// `CHAT`.
    Chat,
    /// `CATEGORY_PERSONAL`.
    CategoryPersonal,
    /// `CATEGORY_SOCIAL`.
    CategorySocial,
    /// `CATEGORY_PROMOTIONS`.
    CategoryPromotions,
    /// `CATEGORY_UPDATES`.
    CategoryUpdates,
    /// `CATEGORY_FORUMS`.
    CategoryForums,
}

impl SystemLabel {
    /// Every system label, in declaration order.
    pub const ALL: [Self; 14] = [
        Self::Inbox,
        Self::Sent,
        Self::Draft,
        Self::Starred,
        Self::Unread,
        Self::Important,
        Self::Trash,
        Self::Spam,
        Self::Chat,
        Self::CategoryPersonal,
        Self::CategorySocial,
        Self::CategoryPromotions,
        Self::CategoryUpdates,
        Self::CategoryForums,
    ];

    /// The labels a new account's label table starts with, with their
    /// display names. Gmail's `labels.list` adds the rest on first sync.
    pub const SEEDED: [(Self, &'static str); 10] = [
        (Self::Inbox, "Inbox"),
        (Self::Sent, "Sent"),
        (Self::Draft, "Drafts"),
        (Self::Starred, "Starred"),
        (Self::Unread, "Unread"),
        (Self::Important, "Important"),
        (Self::Trash, "Trash"),
        (Self::Spam, "Spam"),
        (Self::CategoryPersonal, "Personal"),
        (Self::CategoryPromotions, "Promotions"),
    ];

    /// Gmail's id for the label, which is also its local id.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Inbox => "INBOX",
            Self::Sent => "SENT",
            Self::Draft => "DRAFT",
            Self::Starred => "STARRED",
            Self::Unread => "UNREAD",
            Self::Important => "IMPORTANT",
            Self::Trash => "TRASH",
            Self::Spam => "SPAM",
            Self::Chat => "CHAT",
            Self::CategoryPersonal => "CATEGORY_PERSONAL",
            Self::CategorySocial => "CATEGORY_SOCIAL",
            Self::CategoryPromotions => "CATEGORY_PROMOTIONS",
            Self::CategoryUpdates => "CATEGORY_UPDATES",
            Self::CategoryForums => "CATEGORY_FORUMS",
        }
    }

    /// The label as a [`LabelId`].
    pub fn label_id(self) -> LabelId {
        LabelId::from(self.id())
    }

    /// The system label with exactly this id, if any.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.id() == id)
    }
}

impl fmt::Display for SystemLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Whether `id` starts with `CATEGORY_`, ignoring ASCII case.
pub fn is_category_id(id: &str) -> bool {
    id.get(..CATEGORY_PREFIX.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(CATEGORY_PREFIX))
}

/// Whether `id` names a Gmail system label or any category, ignoring case and
/// surrounding spaces. Everything else is a user label's id or name.
pub fn is_system_label_id(id: &str) -> bool {
    let id = id.trim();
    is_category_id(id)
        || SystemLabel::ALL
            .iter()
            .filter(|l| !is_category_id(l.id()))
            .any(|l| l.id().eq_ignore_ascii_case(id))
}

/// A set of label ids, as on a message or a thread. Ordered, so it stores
/// and compares the same however it was built. Serializes as an array.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LabelSet(BTreeSet<LabelId>);

impl LabelSet {
    /// The empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the set holds the label with this id.
    pub fn contains(&self, id: &str) -> bool {
        self.0.contains(id)
    }

    /// Whether the set holds this system label.
    pub fn has(&self, label: SystemLabel) -> bool {
        self.contains(label.id())
    }

    /// Adds a label; false when it was already there.
    pub fn insert(&mut self, id: LabelId) -> bool {
        self.0.insert(id)
    }

    /// Removes a label; false when it was not there.
    pub fn remove(&mut self, id: &str) -> bool {
        self.0.remove(id)
    }

    /// The labels, in order.
    pub fn iter(&self) -> impl Iterator<Item = &LabelId> + '_ {
        self.0.iter()
    }

    /// How many labels the set holds.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The labels of `self` that are not in `other`.
    #[must_use]
    pub fn without(&self, other: &Self) -> Self {
        self.0.difference(&other.0).cloned().collect()
    }

    /// The labels in both `self` and `other`.
    #[must_use]
    pub fn and(&self, other: &Self) -> Self {
        self.0.intersection(&other.0).cloned().collect()
    }
}

impl FromIterator<LabelId> for LabelSet {
    fn from_iter<I: IntoIterator<Item = LabelId>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<'a> FromIterator<&'a str> for LabelSet {
    fn from_iter<I: IntoIterator<Item = &'a str>>(iter: I) -> Self {
        iter.into_iter().map(LabelId::from).collect()
    }
}

impl FromIterator<SystemLabel> for LabelSet {
    fn from_iter<I: IntoIterator<Item = SystemLabel>>(iter: I) -> Self {
        iter.into_iter().map(SystemLabel::label_id).collect()
    }
}

impl Extend<LabelId> for LabelSet {
    fn extend<I: IntoIterator<Item = LabelId>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}

impl IntoIterator for LabelSet {
    type Item = LabelId;
    type IntoIter = std::collections::btree_set::IntoIter<LabelId>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a LabelSet {
    type Item = &'a LabelId;
    type IntoIter = std::collections::btree_set::Iter<'a, LabelId>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_ids_round_trip() {
        for label in SystemLabel::ALL {
            assert_eq!(SystemLabel::from_id(label.id()), Some(label));
        }
        assert_eq!(SystemLabel::from_id("inbox"), None, "ids are exact");
        assert_eq!(SystemLabel::from_id("Label_1"), None);
    }

    #[test]
    fn system_label_ids_ignore_case_and_spaces() {
        assert!(is_system_label_id("inbox"));
        assert!(is_system_label_id(" CHAT "));
        assert!(is_system_label_id("category_purchases"));
        assert!(!is_system_label_id("Label_7"));
        assert!(!is_system_label_id("Clients"));
        assert!(!is_system_label_id("CATEGORY"));
    }

    #[test]
    fn set_operations() {
        let a: LabelSet = ["INBOX", "UNREAD", "Label_1"].into_iter().collect();
        let b: LabelSet = ["UNREAD", "STARRED"].into_iter().collect();
        assert!(a.has(SystemLabel::Inbox));
        assert!(!a.has(SystemLabel::Starred));
        assert_eq!(a.without(&b), ["INBOX", "Label_1"].into_iter().collect());
        assert_eq!(a.and(&b), ["UNREAD"].into_iter().collect());
    }
}
