use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::LabelId;

const CATEGORY_PREFIX: &str = "CATEGORY_";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SystemLabel {
    Inbox,
    Sent,
    Draft,
    Starred,
    Unread,
    Important,
    Trash,
    Spam,
    Chat,
    CategoryPersonal,
    CategorySocial,
    CategoryPromotions,
    CategoryUpdates,
    CategoryForums,
}

impl SystemLabel {
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

    #[must_use]
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

    #[must_use]
    pub fn label_id(self) -> LabelId {
        LabelId::from(self.id())
    }
}

impl fmt::Display for SystemLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

pub(crate) fn is_category_id(id: &str) -> bool {
    id.get(..CATEGORY_PREFIX.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(CATEGORY_PREFIX))
}

pub(crate) fn is_system_label_id(id: &str) -> bool {
    let id = id.trim();
    is_category_id(id)
        || SystemLabel::ALL
            .iter()
            .filter(|l| !is_category_id(l.id()))
            .any(|l| l.id().eq_ignore_ascii_case(id))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LabelSet(BTreeSet<LabelId>);

impl LabelSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.0.contains(id)
    }

    #[must_use]
    pub fn has(&self, label: SystemLabel) -> bool {
        self.contains(label.id())
    }

    pub fn insert(&mut self, id: LabelId) -> bool {
        self.0.insert(id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.0.remove(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &LabelId> + '_ {
        self.0.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn without(&self, other: &Self) -> Self {
        self.0.difference(&other.0).cloned().collect()
    }

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
