//! The command surface (ARCHITECTURE.md, section 2), defined once: each
//! command's name, its arguments, and its reply. Both hosts (the Tauri
//! commands and the debug web bridge) parse an invoke into a [`Command`]
//! and answer it through its [`Replier`]; the page's `invoke` is typed from
//! the bindings of [`Command`] and [`CommandReply`]. Also the results the
//! commands reply with, and the app events the shell sends the page.

use std::fmt;
use std::marker::PhantomData;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sm_domain::{AccountId, ActionNo, AttachmentId, DraftId, MessageId, Millis, ThreadId};
use ts_rs::TS;

use crate::{DraftAttachment, ListRow, MessageBody, NumberedAction, Submitted};

/// Defines every command from one list: [`Command`], what the hosts parse
/// an invoke into, and [`CommandReply`], what each command answers, for the
/// page. A command's arguments are named in camelCase, as the page names
/// them.
macro_rules! commands {
    ($(
        $(#[doc = $doc:literal])+
        $command:ident { $($(#[$attr:meta])* $arg:ident: $ty:ty),* $(,)? } -> $reply:ty;
    )+) => {
        /// A command the page invokes, by name, with its arguments.
        #[derive(Deserialize, TS)]
        #[serde(tag = "command", content = "args", rename_all = "snake_case")]
        #[ts(export)]
        pub enum Command {
            $(
                $(#[doc = $doc])+
                #[serde(rename_all = "camelCase")]
                $command {
                    $($(#[$attr])* $arg: $ty,)*
                    /// Answers the command, with its own reply type only.
                    #[serde(skip)]
                    #[ts(skip)]
                    reply: Replier<$reply>,
                },
            )+
        }

        /// What each command replies, by name. Never built: it types the
        /// page's invoke, and the core replies through each command's `Replier`.
        #[derive(TS)]
        #[ts(export, tag = "command", content = "reply", rename_all = "snake_case")]
        pub enum CommandReply {
            $(
                $(#[doc = $doc])+
                $command($reply),
            )+
        }

        /// The command's name only: its arguments can hold the user's text
        /// and files, which never go to the logs.
        impl fmt::Debug for Command {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(match self {
                    $(Self::$command { .. } => stringify!($command),)+
                })
            }
        }

        /// The command's name only: a reply can hold mail.
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
    /// The page's first call; returns where its action numbers continue.
    /// Each host connects the page's channel its own way.
    Connect {} -> LaunchInfo;
    /// Queues the page's numbered actions. Returns once they are queued.
    /// Each action is read on its own (`Submitted`).
    Submit {
        /// The actions taken since the last invoke, in number order.
        #[ts(as = "Vec<NumberedAction>")]
        actions: Vec<Submitted>,
    } -> ();
    /// Rows outside the pushed window, with their `rev`.
    FetchRows {
        /// The keys are draft ids, as the Drafts list's are, not thread ids.
        drafts: bool,
        /// The list keys to read. An unknown one is left out of the reply.
        keys: Vec<String>,
    } -> Vec<ListRow>;
    /// Sanitized bodies. A message without a stored body is left out.
    GetBodies {
        /// The messages whose bodies to read.
        ids: Vec<MessageId>,
    } -> Vec<MessageBody>;
    /// Recipient suggestions for what the user typed.
    Autocomplete {
        /// The account whose contacts are searched.
        account: AccountId,
        /// The text typed in the recipient field. Blank text suggests nothing.
        query: String,
    } -> Vec<ContactSuggestion>;
    /// Signs a new account in with Google in the browser and adds it.
    AddAccount {} -> AccountId;
    /// Signs an account in again.
    SignInAgain {
        /// The account to sign in. The browser must sign in that same Google
        /// account.
        account: AccountId,
    } -> ();
    /// Downloads an attachment if needed and opens it with its default app.
    OpenAttachment {
        /// The attachment whose chip the user clicked.
        attachment: AttachmentId,
    } -> ();
    /// Opens a link from a message in the default browser or mail app.
    OpenUrl {
        /// The link. Only an `http`, `https`, or `mailto` one opens.
        url: String,
    } -> ();
    /// Unsubscribes from the list a conversation came from.
    Unsubscribe {
        /// The account the conversation is in, fixed when the user chose.
        account: AccountId,
        /// The conversation. Its latest list message names the list, else
        /// its latest message.
        thread: ThreadId,
    } -> Unsubscribed;
    /// Blocks a conversation's sender, or the sender's domain.
    Block {
        /// The account that gets the Gmail filter, fixed when the user chose.
        account: AccountId,
        /// The conversation whose latest message names the sender.
        thread: ThreadId,
        /// Block everyone at the sender's domain, not only the sender.
        domain: bool,
    } -> BlockResult;
    /// The account's blocked senders.
    BlockedSenders {
        /// The account whose Gmail filters are listed.
        account: AccountId,
    } -> BlockedSenders;
    /// Unblocks a sender by deleting its Gmail filter `id`.
    Unblock {
        /// The account that has the filter.
        account: AccountId,
        /// The filter's id, from `blocked_senders`. Any other filter of the
        /// account is refused.
        id: String,
    } -> ();
    /// Inbox conversations from the sender of `thread`'s latest message.
    SenderThreads {
        /// The account whose Inbox is searched.
        account: AccountId,
        /// The conversation that names the sender. It is in the reply too.
        thread: ThreadId,
    } -> Vec<ThreadId>;
    /// Keeps a file the user picked with the draft's files. `data` is the
    /// file's content in base64, as the page reads it.
    AddAttachment {
        /// The draft the file is added to. One already gone keeps nothing.
        draft: DraftId,
        /// The file's name, as the user picked it.
        filename: String,
        /// The file's MIME type, as the page's file picker reports it.
        mime_type: String,
        /// The file's content in base64, at most `maxAttachmentBytes` once
        /// decoded.
        data: String,
    } -> DraftAttachment;
    /// Quits once the core has committed the page's last actions, after the
    /// page has closed an open composer (SPEC Send and undo: Cmd+Q).
    QuitNow {} -> ();
    /// Debug builds: the keydown-to-paint meter reports how long an action took.
    PerfReport {
        /// The shortcut or action measured, by the meter's name for it.
        action: String,
        /// Keydown to the frame that showed the result, in milliseconds.
        ms: f64,
        /// The action's budget in milliseconds (SPEC Performance). A time
        /// over it is logged as a warning.
        budget: f64,
    } -> ();
    /// The snooze picker's choices for what the user typed.
    SnoozeOptions {
        /// The text typed in the snooze picker; empty lists the fixed
        /// choices.
        text: String,
    } -> Vec<SnoozeChoice>;
}

impl Command {
    /// The command an invoke names, with its arguments as the page sent
    /// them: an object, empty for a command that takes none.
    pub fn parse(name: &str, args: Value) -> Result<Self, String> {
        let invoke = serde_json::Map::from_iter([
            ("command".to_owned(), Value::String(name.to_owned())),
            ("args".to_owned(), args),
        ]);
        Self::deserialize(Value::Object(invoke)).map_err(|e| format!("{name}: {e}"))
    }
}

/// Answers one command, with the reply type its command declares and no
/// other, so a host can't answer `fetch_rows` with what `get_bodies` gives.
pub struct Replier<T>(PhantomData<fn(T)>);

/// Its name only: it holds no value, and `T` need not be `Debug`.
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
    /// The command's reply, as JSON for the page.
    #[expect(
        clippy::unused_self,
        reason = "replying takes the replier, so a command is answered once"
    )]
    pub fn send(self, value: T) -> Result<Reply, serde_json::Error> {
        serde_json::to_value(value).map(Reply)
    }
}

/// A command's reply as JSON, made only by its command's [`Replier`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply(Value);

impl Reply {
    /// The JSON the page receives.
    #[must_use]
    pub fn into_json(self) -> Value {
        self.0
    }
}

/// What the shell tells the page outside the channel: Tauri's `emit` in the
/// app, a named server-sent event on the web bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "event", content = "payload", rename_all = "kebab-case")]
#[ts(export)]
pub enum AppEvent {
    /// A notification was clicked: that account's Inbox, the thread highlighted.
    NotificationOpen {
        /// The account the notification was for.
        account: AccountId,
        /// The thread to highlight. `None` for a summary of several
        /// messages.
        thread: Option<ThreadId>,
    },
    /// Cmd+Q: the page closes an open composer, then invokes `quit_now`.
    QuitRequested,
}

impl AppEvent {
    /// The event's name and payload, as the page listens for them.
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

/// What `connect` tells a fresh page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaunchInfo {
    /// Continue numbering actions after this.
    pub last_action: ActionNo,
    /// A debug build: the page runs the keydown-to-paint meter.
    pub debug: bool,
    /// The largest file a draft takes, in bytes: the page refuses a larger
    /// one before reading it (SPEC Drafts).
    pub max_attachment_bytes: u32,
}

/// One `autocomplete` match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContactSuggestion {
    /// The contact's latest display name; empty when it has none.
    pub name: String,
    /// The contact's address.
    pub email: String,
    /// Which of the two the query matched, for bolding: `name` or `email`.
    pub match_field: String,
    /// The first matched byte of that field, as UTF-8.
    pub match_start: u32,
    /// The byte past the last matched one.
    pub match_end: u32,
}

/// One `snooze_options` choice: its label and when it wakes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SnoozeChoice {
    /// What the picker shows: the time, after the choice's name for a
    /// fixed one, such as `Next week · Mon Nov 9, 8:00 AM`.
    pub label: String,
    /// When the snooze ends.
    pub at: Millis,
}

/// What an unsubscribe did (SPEC: Unsubscribe and block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Unsubscribed {
    /// The list's one-click URL accepted the request.
    OneClick {
        /// The list's sender, by name or else address, for the toast.
        sender: String,
    },
    /// An unsubscribe email went to the list.
    Emailed {
        /// The list's sender, by name or else address, for the toast.
        sender: String,
    },
    /// The list's unsubscribe page opened in the browser.
    OpenedPage {
        /// The list's sender, by name or else address, for the toast.
        sender: String,
    },
    /// The message offers no way to unsubscribe.
    NoLink {
        /// The message's sender, by name or else address, for the toast.
        sender: String,
    },
}

/// What Block did (SPEC: Unsubscribe and block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum BlockResult {
    /// A Gmail filter now sends mail from `from` (an address or `@domain`) to Trash.
    Blocked {
        /// What the filter matches: the sender's address, or `@domain`.
        from: String,
    },
    /// The account's grant lacks Gmail filters; Sign in again adds it.
    NeedsPermission,
}

/// A sender or domain a Gmail filter sends to Trash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BlockedSender {
    /// The Gmail filter id.
    pub id: String,
    /// An address, or `@domain`.
    pub from: String,
}

/// The account's blocked senders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum BlockedSenders {
    /// The filters that block senders.
    List {
        /// One per filter that matches only a sender and sends it to Trash.
        senders: Vec<BlockedSender>,
    },
    /// The account's grant lacks Gmail filters; Sign in again adds it.
    NeedsPermission,
}
