use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use super::LabelId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Addr {
    pub name: Option<String>,
    pub email: String,
}

impl Addr {
    #[must_use]
    pub fn new(email: impl Into<String>) -> Self {
        Self {
            name: None,
            email: email.into(),
        }
    }

    #[must_use]
    pub fn named(name: impl Into<String>, email: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            name: (!name.is_empty()).then_some(name),
            email: email.into(),
        }
    }

    #[must_use]
    pub fn key(&self) -> String {
        self.email.trim().to_ascii_lowercase()
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.email)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendAsAddress {
    pub email: String,
    pub display_name: String,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    primary: String,
    display_name: String,
    send_as: Vec<SendAsAddress>,
}

impl Identity {
    #[must_use]
    pub fn new(
        primary: impl Into<String>,
        display_name: impl Into<String>,
        send_as: Vec<SendAsAddress>,
    ) -> Self {
        Self {
            primary: primary.into(),
            display_name: display_name.into(),
            send_as,
        }
    }

    #[must_use]
    pub fn primary_email(&self) -> &str {
        &self.primary
    }

    #[must_use]
    pub fn is_mine(&self, email: &str) -> bool {
        let trimmed = email.trim();
        self.primary.eq_ignore_ascii_case(trimmed)
            || self
                .send_as
                .iter()
                .any(|s| s.email.trim().eq_ignore_ascii_case(trimmed))
    }

    #[must_use]
    pub fn send_as_for(&self, email: &str) -> Option<Addr> {
        let trimmed = email.trim();
        if let Some(found) = self
            .send_as
            .iter()
            .find(|s| s.email.trim().eq_ignore_ascii_case(trimmed))
        {
            let name = if found.display_name.is_empty() {
                self.display_name.as_str()
            } else {
                found.display_name.as_str()
            };
            return Some(Addr::named(name, found.email.clone()));
        }
        if self.primary.eq_ignore_ascii_case(trimmed) {
            return Some(Addr::named(self.display_name.clone(), self.primary.clone()));
        }
        None
    }

    #[must_use]
    pub fn default_from(&self) -> Addr {
        if let Some(primary) = self.send_as.iter().find(|s| s.primary) {
            let name = if primary.display_name.is_empty() {
                self.display_name.as_str()
            } else {
                primary.display_name.as_str()
            };
            return Addr::named(name, primary.email.clone());
        }
        Addr::named(self.display_name.clone(), self.primary.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemLabel {
    Inbox,
    Unread,
    Starred,
    Sent,
    Draft,
    Trash,
    Spam,
    Important,
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
        Self::Unread,
        Self::Starred,
        Self::Sent,
        Self::Draft,
        Self::Trash,
        Self::Spam,
        Self::Important,
        Self::Chat,
        Self::CategoryPersonal,
        Self::CategorySocial,
        Self::CategoryPromotions,
        Self::CategoryUpdates,
        Self::CategoryForums,
    ];

    pub const SEEDED: [Self; 13] = [
        Self::Inbox,
        Self::Unread,
        Self::Starred,
        Self::Sent,
        Self::Draft,
        Self::Trash,
        Self::Spam,
        Self::Important,
        Self::CategoryPersonal,
        Self::CategorySocial,
        Self::CategoryPromotions,
        Self::CategoryUpdates,
        Self::CategoryForums,
    ];

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Inbox => "INBOX",
            Self::Unread => "UNREAD",
            Self::Starred => "STARRED",
            Self::Sent => "SENT",
            Self::Draft => "DRAFT",
            Self::Trash => "TRASH",
            Self::Spam => "SPAM",
            Self::Important => "IMPORTANT",
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
        LabelId(self.id().to_owned())
    }

    #[must_use]
    pub fn is_system_id(id: &str) -> bool {
        Self::ALL.iter().any(|l| l.id().eq_ignore_ascii_case(id))
    }

    #[must_use]
    pub fn is_non_personal_category(id: &str) -> bool {
        matches!(
            id.to_ascii_uppercase().as_str(),
            "CATEGORY_PROMOTIONS" | "CATEGORY_SOCIAL" | "CATEGORY_UPDATES" | "CATEGORY_FORUMS"
        )
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LabelSet(BTreeSet<LabelId>);

impl LabelSet {
    #[must_use]
    pub const fn new() -> Self {
        Self(BTreeSet::new())
    }

    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.0.iter().any(|l| l.0 == id)
    }

    #[must_use]
    pub fn has(&self, label: SystemLabel) -> bool {
        self.contains(label.id())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn insert(&mut self, label: LabelId) -> bool {
        self.0.insert(label)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let len_before = self.0.len();
        self.0.retain(|l| l.0 != id);
        self.0.len() != len_before
    }

    #[must_use]
    pub fn without(mut self, labels: &[LabelId]) -> Self {
        for l in labels {
            self.0.remove(l);
        }
        self
    }

    #[must_use]
    pub fn and(mut self, labels: &[LabelId]) -> Self {
        for l in labels {
            if !self.0.contains(l) {
                self.0.insert(l.clone());
            }
        }
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = &LabelId> {
        self.0.iter()
    }
}

impl FromIterator<LabelId> for LabelSet {
    fn from_iter<T: IntoIterator<Item = LabelId>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl<'a> IntoIterator for &'a LabelSet {
    type Item = &'a LabelId;
    type IntoIter = std::collections::btree_set::Iter<'a, LabelId>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl IntoIterator for LabelSet {
    type Item = LabelId;
    type IntoIter = std::collections::btree_set::IntoIter<LabelId>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown {kind}: {value:?}")]
pub struct UnknownKind {
    pub kind: &'static str,
    pub value: String,
}

macro_rules! db_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident ($label:literal) {
            $( $variant:ident => $text:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub enum $name {
            $($variant,)+
        }

        impl $name {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = UnknownKind;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($text => Ok(Self::$variant),)+
                    other => Err(UnknownKind {
                        kind: $label,
                        value: other.to_owned(),
                    }),
                }
            }
        }
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum View {
    Inbox,
    Done,
    AllMail,
    Snoozed,
    Starred,
    Sent,
    Drafts,
    Trash,
    Spam,
    Label { label: LabelId },
}

db_enum! {
    #[derive(TS)]
    #[ts(export)]
    pub enum DraftKind("draft kind") {
        New => "new",
        Reply => "reply",
        Forward => "forward",
    }
}

db_enum! {
    #[derive(TS)]
    #[ts(export)]
    pub enum OutboxState("outbox state") {
        Waiting => "waiting",
        Sending => "sending",
        Sent => "sent",
    }
}

db_enum! {
    #[derive(TS)]
    #[ts(export)]
    pub enum SplitKind("split kind") {
        Custom => "custom",
        Important => "important",
        Other => "other",
    }
}

db_enum! {
    #[derive(TS)]
    #[ts(export)]
    pub enum PinKind("pin kind") {
        Sender => "sender",
        Domain => "domain",
        Label => "label",
    }
}

db_enum! {
    #[derive(TS)]
    #[ts(export)]
    pub enum SyncStatus("sync status") {
        Idle => "idle",
        Syncing => "syncing",
        Offline => "offline",
        Retrying => "retrying",
        NeedsSignin => "needs_signin",
    }
}

db_enum! {
    #[derive(TS)]
    #[ts(export)]
    pub enum NoticeKind("notice kind") {
        Rejected => "rejected",
        SendFailed => "send_failed",
        SendUndone => "send_undone",
        DraftGone => "draft_gone",
    }
}

db_enum! {
    #[derive(TS)]
    #[ts(export)]
    pub enum ModifyKind("modify kind") {
        Done => "done",
        ReturnToInbox => "return_to_inbox",
        Snooze => "snooze",
        Trash => "trash",
        Spam => "spam",
        Star => "star",
        Unstar => "unstar",
        Read => "read",
        Unread => "unread",
        Label => "label",
        RemoveLabel => "remove_label",
    }
}

db_enum! {
    pub enum SettingKey("setting key") {
        RemoteImages => "remote_images",
        Muted => "muted",
    }
}

db_enum! {
    pub enum OpKind("op kind") {
        Modify => "modify",
        CreateLabel => "create_label",
        DraftUpsert => "draft_upsert",
        DraftDelete => "draft_delete",
        TrashMessage => "trash_message",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpQueue {
    Mail,
    Drafts,
}

db_enum! {
    pub enum DraftWait("draft wait") {
        Parts => "parts",
        TargetBody => "target_body",
    }
}
