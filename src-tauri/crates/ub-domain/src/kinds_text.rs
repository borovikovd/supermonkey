//! Text forms of the small enums, for storage: `as_str`, `FromStr`, and with
//! the `sqlite` feature `ToSql` and `FromSql` as that text.

use std::fmt;
use std::str::FromStr;

use crate::{
    DraftKind, ModifyKind, NoticeKind, OutboxState, PinKind, SettingKey, SplitKind, SyncStatus,
    UndoKind,
};

/// Stored text that names no value of the enum it was read as.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind} {value:?}")]
pub struct UnknownKind {
    /// The enum's name.
    pub kind: &'static str,
    /// The text read.
    pub value: String,
}

macro_rules! text_enum {
    ($($ty:ident { $($variant:ident => $text:literal),+ $(,)? })*) => {$(
        impl $ty {
            /// The value's stored text.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $ty {
            type Err = UnknownKind;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($text => Ok(Self::$variant),)+
                    _ => Err(UnknownKind {
                        kind: stringify!($ty),
                        value: s.to_owned(),
                    }),
                }
            }
        }

        #[cfg(feature = "sqlite")]
        impl rusqlite::ToSql for $ty {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(rusqlite::types::ToSqlOutput::from(self.as_str()))
            }
        }

        #[cfg(feature = "sqlite")]
        impl rusqlite::types::FromSql for $ty {
            fn column_result(
                v: rusqlite::types::ValueRef<'_>,
            ) -> rusqlite::types::FromSqlResult<Self> {
                v.as_str()?
                    .parse()
                    .map_err(|e| rusqlite::types::FromSqlError::Other(Box::new(e)))
            }
        }
    )*};
}

text_enum! {
    DraftKind { New => "new", Reply => "reply", Forward => "forward" }
    OutboxState { Waiting => "waiting", Sending => "sending", Sent => "sent" }
    SplitKind { Custom => "custom", Important => "important", Other => "other" }
    PinKind { Sender => "sender", Domain => "domain", Label => "label" }
    SyncStatus {
        Idle => "idle",
        Syncing => "syncing",
        Offline => "offline",
        Retrying => "retrying",
        NeedsSignin => "needs_signin",
    }
    UndoKind { Mail => "mail", Send => "send", Pin => "pin" }
    NoticeKind {
        Rejected => "rejected",
        SendFailed => "send_failed",
        SendUndone => "send_undone",
        DraftGone => "draft_gone",
        Info => "info",
    }
    ModifyKind {
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
    SettingKey { RemoteImages => "remote_images", Muted => "muted" }
}

impl ModifyKind {
    /// What the undo toast and history call the action.
    pub const fn undo_label(self) -> &'static str {
        match self {
            Self::Done => "Marked done",
            Self::ReturnToInbox => "Moved to inbox",
            Self::Snooze => "Snoozed",
            Self::Trash => "Moved to trash",
            Self::Spam => "Marked as spam",
            Self::Star => "Starred",
            Self::Unstar => "Unstarred",
            Self::Read => "Marked read",
            Self::Unread => "Marked unread",
            Self::Label => "Labeled",
            Self::RemoveLabel => "Label removed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_and_unknown_text_is_an_error() {
        for kind in [DraftKind::New, DraftKind::Reply, DraftKind::Forward] {
            assert_eq!(kind.as_str().parse::<DraftKind>(), Ok(kind));
        }
        assert_eq!("needs_signin".parse(), Ok(SyncStatus::NeedsSignin));
        assert_eq!(
            "Reply".parse::<DraftKind>(),
            Err(UnknownKind {
                kind: "DraftKind",
                value: "Reply".into()
            })
        );
    }
}
