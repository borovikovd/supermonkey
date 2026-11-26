use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, de};

macro_rules! int_id {
    ($name:ident) => {
        #[derive(
            Debug,
            Default,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

macro_rules! str_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

int_id!(AccountId);
int_id!(ThreadId);
int_id!(MessageId);
int_id!(SplitId);
int_id!(OutboxId);
int_id!(UndoEntryId);
int_id!(NoticeId);
int_id!(AttachmentId);
int_id!(Rev);
int_id!(ActionNo);
int_id!(OpId);
int_id!(Millis);

impl SplitId {
    #[must_use]
    pub const fn is_saved(self) -> bool {
        self.0 > 0
    }
}

impl Millis {
    #[must_use]
    pub fn plus(self, duration: Duration) -> Self {
        let delta = i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
        Self(self.0.saturating_add(delta))
    }

    #[must_use]
    pub const fn since(self, earlier: Self) -> i64 {
        self.0.saturating_sub(earlier.0)
    }

    #[must_use]
    pub fn duration_since(self, earlier: Self) -> Duration {
        let diff = self.0.saturating_sub(earlier.0);
        u64::try_from(diff).map_or(Duration::ZERO, Duration::from_millis)
    }
}

str_id!(DraftId);
str_id!(SubId);
str_id!(LabelId);
str_id!(GmailThreadId);
str_id!(GmailMessageId);
str_id!(GmailDraftId);
str_id!(GmailAttachmentId);
str_id!(PageToken);
str_id!(FilterId);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct HistoryId(pub u64);

impl fmt::Display for HistoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for HistoryId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Int(u64),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Int(n) => Ok(Self(n)),
            Raw::Text(s) => s.parse::<u64>().map(Self).map_err(|_parse| {
                de::Error::invalid_value(de::Unexpected::Str(&s), &"a u64 historyId")
            }),
        }
    }
}
