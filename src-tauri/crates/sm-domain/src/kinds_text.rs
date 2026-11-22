use std::fmt;
use std::str::FromStr;

use crate::{
    DraftKind, DraftWait, ModifyKind, NoticeKind, OpKind, OutboxState, PinKind, SettingKey,
    SplitKind, SyncStatus,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind} {value:?}")]
pub struct UnknownKind {
    pub kind: &'static str,
    pub value: String,
}

macro_rules! text_enum {
    ($($ty:ident { $($variant:ident => $text:literal),+ $(,)? })*) => {$(
        impl $ty {
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
    NoticeKind {
        Rejected => "rejected",
        SendFailed => "send_failed",
        SendUndone => "send_undone",
        DraftGone => "draft_gone",
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
    OpKind {
        Modify => "modify",
        LabelCreate => "label_create",
        DraftUpsert => "draft_upsert",
        DraftDelete => "draft_delete",
        TrashMessage => "trash_message",
    }
    DraftWait { Parts => "parts", TargetBody => "target_body" }
}
