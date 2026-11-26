mod actions;
mod subs;
mod types;

use std::fmt;
use std::marker::PhantomData;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::domain::{
    AccountId, ActionNo, AttachmentId, DraftId, LabelId, MessageId, Millis, NoticeId, OutboxId,
    Rev, SplitId, SubId, ThreadId, UndoEntryId,
};

pub use crate::domain::{
    Addr, DraftKind, ModifyKind, NoticeKind, OutboxState, PinKind, SplitKind, SyncStatus, View,
};
pub use actions::{
    Action, ModifyAction, NumberedAction, Setting, SnoozeChange, Submitted, Unreadable,
};
pub use subs::{ChannelMessage, RejectReason, Rejection, SubKey, SubUpdate};
pub use types::{
    AccountInfo, AccountSummary, AppState, AttachmentInfo, ConversationData, DraftAttachment,
    DraftFields, DraftSave, DraftState, DraftSummary, GlobalSettings, LabelInfo, ListEntry,
    ListMove, ListRow, MessageAddr, MessageBody, MessageSummary, Notice, OutboxRow, SendAs,
    SplitInfo, UndoEntryInfo, avatar_hue,
};

macro_rules! impl_ts_primitive {
    ($prim:literal : $($ty:ty),+ $(,)?) => {
        $(
            impl TS for $ty {
                type WithoutGenerics = Self;
                type OptionInnerType = Self;

                fn name(_cfg: &ts_rs::Config) -> String {
                    $prim.to_owned()
                }

                fn inline(_cfg: &ts_rs::Config) -> String {
                    $prim.to_owned()
                }
            }
        )+
    };
}

impl_ts_primitive!(
    "number":
    AccountId,
    ThreadId,
    MessageId,
    SplitId,
    OutboxId,
    UndoEntryId,
    NoticeId,
    AttachmentId,
    Rev,
    ActionNo,
    Millis
);

impl_ts_primitive!("string": DraftId, SubId, LabelId);

macro_rules! commands {
    ($(
        $command:ident { $($(#[$attr:meta])* $arg:ident: $ty:ty),* $(,)? } -> $reply:ty;
    )+) => {
        #[derive(Deserialize, TS)]
        #[serde(tag = "command", content = "args", rename_all = "snake_case")]
        #[ts(export)]
        pub enum Command {
            $(
                #[serde(rename_all = "camelCase")]
                $command {
                    $($(#[$attr])* $arg: $ty,)*
                    #[serde(skip)]
                    #[ts(skip)]
                    reply: Replier<$reply>,
                },
            )+
        }

        #[derive(TS)]
        #[ts(export, tag = "command", content = "reply", rename_all = "snake_case")]
        pub enum CommandReply {
            $(
                $command($reply),
            )+
        }

        impl fmt::Debug for Command {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(match self {
                    $(Self::$command { .. } => stringify!($command),)+
                })
            }
        }

        impl fmt::Debug for CommandReply {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(match self {
                    $(Self::$command(_) => stringify!($command),)+
                })
            }
        }
    };
}

commands! {
    Connect {} -> LaunchInfo;
    Submit {
        #[ts(as = "Vec<NumberedAction>")]
        actions: Vec<Submitted>,
    } -> ();
    FetchRows {
        drafts: bool,
        keys: Vec<String>,
    } -> Vec<ListRow>;
    GetBodies {
        ids: Vec<MessageId>,
    } -> Vec<MessageBody>;
    Autocomplete {
        account: AccountId,
        query: String,
    } -> Vec<ContactSuggestion>;
    AddAccount {} -> AccountId;
    SignInAgain {
        account: AccountId,
    } -> ();
    OpenAttachment {
        attachment: AttachmentId,
    } -> ();
    OpenUrl {
        url: String,
    } -> ();
    Unsubscribe {
        account: AccountId,
        thread: ThreadId,
    } -> Unsubscribed;
    Block {
        account: AccountId,
        thread: ThreadId,
        domain: bool,
    } -> BlockResult;
    BlockedSenders {
        account: AccountId,
    } -> BlockedSenders;
    Unblock {
        account: AccountId,
        id: String,
    } -> ();
    SenderThreads {
        account: AccountId,
        thread: ThreadId,
    } -> Vec<ThreadId>;
    AddAttachment {
        draft: DraftId,
        filename: String,
        mime_type: String,
        data: String,
    } -> DraftAttachment;
    QuitNow {} -> ();
    PerfReport {
        action: String,
        ms: f64,
        budget: f64,
    } -> ();
    SnoozeOptions {
        text: String,
    } -> Vec<SnoozeChoice>;
}

impl Command {
    pub fn parse(name: &str, args: Value) -> Result<Self, String> {
        let invoke = serde_json::Map::from_iter([
            ("command".to_owned(), Value::String(name.to_owned())),
            ("args".to_owned(), args),
        ]);
        Self::deserialize(Value::Object(invoke)).map_err(|e| format!("{name}: {e}"))
    }
}

pub struct Replier<T>(PhantomData<fn(T)>);

impl<T> fmt::Debug for Replier<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Replier")
    }
}

impl<T> Default for Replier<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: Serialize> Replier<T> {
    #[expect(
        clippy::unused_self,
        reason = "replying consumes the replier so each command answers once"
    )]
    pub fn send(self, value: T) -> Result<Reply, serde_json::Error> {
        serde_json::to_value(value).map(Reply)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply(Value);

impl Reply {
    #[must_use]
    pub fn into_json(self) -> Value {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CommandError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ErrorKind {
    SignInCancelled,
    DraftGone,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "event", content = "payload", rename_all = "kebab-case")]
#[ts(export)]
pub enum AppEvent {
    NotificationOpen {
        account: AccountId,
        thread: Option<ThreadId>,
    },
    QuitRequested,
}

impl AppEvent {
    pub fn parts(&self) -> Result<(String, Value), serde_json::Error> {
        let mut event = serde_json::to_value(self)?;
        let name = if let Some(Value::String(name)) = event.get_mut("event").map(Value::take) {
            name
        } else {
            String::new()
        };
        let payload = event
            .get_mut("payload")
            .map(Value::take)
            .unwrap_or_default();
        Ok((name, payload))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaunchInfo {
    pub last_action: ActionNo,
    pub debug: bool,
    pub max_attachment_bytes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContactSuggestion {
    pub name: String,
    pub email: String,
    pub match_field: String,
    pub match_start: u32,
    pub match_end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SnoozeChoice {
    pub label: String,
    pub at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Unsubscribed {
    OneClick { sender: String },
    Emailed { sender: String },
    OpenedPage { sender: String },
    NoLink { sender: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum BlockResult {
    Blocked { from: String },
    NeedsPermission,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BlockedSender {
    pub id: String,
    pub from: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum BlockedSenders {
    List { senders: Vec<BlockedSender> },
    NeedsPermission,
}
