//! The command surface (ARCHITECTURE.md, section 2), defined once: each
//! command's name, its arguments, and its reply. Both hosts (the Tauri
//! commands and the debug web bridge) parse an invoke into a [`Command`]
//! and answer it through its [`Replier`]; the page's `invoke` is typed from
//! the bindings of [`Command`] and [`CommandReply`]. Also the results the
//! commands reply with, and the app events the shell sends the page.

use std::marker::PhantomData;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;
use ub_domain::{AccountId, ActionNo, AttachmentId, DraftId, MessageId, Millis, ThreadId};

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
        // A command the page invokes, by name, with its arguments.
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
    };
}

commands! {
    /// The page's first call; returns where its action numbers continue.
    /// Each host connects the page's channel its own way.
    Connect {} -> LaunchInfo;
    /// Queues the page's numbered actions. Returns once they are queued.
    /// Each action is read on its own (`Submitted`).
    Submit {
        #[ts(as = "Vec<NumberedAction>")]
        actions: Vec<Submitted>,
    } -> ();
    /// Rows outside the pushed window, with their `rev`.
    FetchRows { drafts: bool, keys: Vec<String> } -> Vec<ListRow>;
    /// Sanitized bodies. A message without a stored body is left out.
    GetBodies { ids: Vec<MessageId> } -> Vec<MessageBody>;
    /// Recipient suggestions for what the user typed.
    Autocomplete { account: AccountId, query: String } -> Vec<ContactSuggestion>;
    /// Signs a new account in with Google in the browser and adds it.
    AddAccount {} -> AccountId;
    /// Signs an account in again.
    SignInAgain { account: AccountId } -> ();
    /// Downloads an attachment if needed and opens it with its default app.
    OpenAttachment { attachment: AttachmentId } -> ();
    /// Opens a link from a message in the default browser or mail app.
    OpenUrl { url: String } -> ();
    /// Unsubscribes from the list a conversation came from.
    Unsubscribe { account: AccountId, thread: ThreadId } -> Unsubscribed;
    /// Blocks a conversation's sender, or the sender's domain.
    Block { account: AccountId, thread: ThreadId, domain: bool } -> BlockResult;
    /// The account's blocked senders.
    BlockedSenders { account: AccountId } -> BlockedSenders;
    /// Unblocks a sender by deleting its Gmail filter `id`.
    Unblock { account: AccountId, id: String } -> ();
    /// Inbox conversations from the sender of `thread`'s latest message.
    SenderThreads { account: AccountId, thread: ThreadId } -> Vec<ThreadId>;
    /// Keeps a file the user picked with the draft's files. `data` is the
    /// file's content in base64, as the page reads it.
    AddAttachment {
        draft: DraftId,
        filename: String,
        mime_type: String,
        data: String,
    } -> DraftAttachment;
    /// Quits once the core has committed the page's last actions, after the
    /// page has closed an open composer (SPEC Send and undo: Cmd+Q).
    QuitNow {} -> ();
    /// Debug builds: the keydown-to-paint meter reports how long an action took.
    PerfReport { action: String, ms: f64, budget: f64 } -> ();
    /// The snooze picker's choices for what the user typed.
    SnoozeOptions { text: String } -> Vec<SnoozeChoice>;
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

impl<T> Default for Replier<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: Serialize> Replier<T> {
    /// The command's reply, as JSON for the page.
    pub fn send(self, value: T) -> Result<Reply, serde_json::Error> {
        serde_json::to_value(value).map(Reply)
    }
}

/// A command's reply as JSON, made only by its command's [`Replier`].
#[derive(Debug, Clone, PartialEq)]
pub struct Reply(Value);

impl Reply {
    /// The JSON the page receives.
    pub fn into_json(self) -> Value {
        self.0
    }
}

/// What the shell tells the page outside the channel: Tauri's `emit` in the
/// app, a named server-sent event on the web bridge.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(tag = "event", content = "payload", rename_all = "kebab-case")]
#[ts(export)]
pub enum AppEvent {
    /// A notification was clicked: that account's Inbox, the thread highlighted.
    NotificationOpen {
        account: AccountId,
        thread: Option<ThreadId>,
    },
    /// Cmd+Q: the page closes an open composer, then invokes `quit_now`.
    QuitRequested,
}

impl AppEvent {
    /// The event's name and payload, as the page listens for them.
    pub fn parts(&self) -> Result<(String, Value), serde_json::Error> {
        let mut event = serde_json::to_value(self)?;
        let name = event
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let payload = event
            .get_mut("payload")
            .map(Value::take)
            .unwrap_or_default();
        Ok((name, payload))
    }
}

// What `connect` tells a fresh page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaunchInfo {
    /// Continue numbering actions after this.
    pub last_action: ActionNo,
    pub debug: bool,
    /// The largest file a draft takes, in bytes: the page refuses a larger
    /// one before reading it (SPEC Drafts).
    pub max_attachment_bytes: u32,
}

// One `autocomplete` match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContactSuggestion {
    pub name: String,
    pub email: String,
    /// Byte range in name or email that matched, for bolding.
    pub match_field: String,
    pub match_start: u32,
    pub match_end: u32,
}

// One `snooze_options` choice: its label and when it wakes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SnoozeChoice {
    pub label: String,
    pub at: Millis,
}

/// What an unsubscribe did (SPEC: Unsubscribe and block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum Unsubscribed {
    /// The list's one-click URL accepted the request.
    OneClick { sender: String },
    /// An unsubscribe email went to the list.
    Emailed { sender: String },
    /// The list's unsubscribe page opened in the browser.
    OpenedPage { sender: String },
    /// The message offers no way to unsubscribe.
    NoLink { sender: String },
}

/// What Block did (SPEC: Unsubscribe and block).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum BlockResult {
    /// A Gmail filter now sends mail from `from` (an address or `@domain`) to Trash.
    Blocked { from: String },
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
    List { senders: Vec<BlockedSender> },
    /// The account's grant lacks Gmail filters; Sign in again adds it.
    NeedsPermission,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names a binding gives its variants under `tag`.
    fn names(binding: &str, tag: &str) -> Vec<String> {
        binding
            .split(&format!("\"{tag}\": \""))
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .map(str::to_owned)
            .collect()
    }

    /// ts-rs and serde each turn the variant names into the page's names:
    /// every name the page may invoke must parse as a command, and name a reply.
    #[test]
    fn the_page_names_each_command_as_rust_parses_it() {
        let config = ts_rs::Config::default();
        let commands = names(&Command::inline(&config), "command");
        assert_ne!(commands, Vec::<String>::new());
        assert_eq!(commands, names(&CommandReply::inline(&config), "command"));
        for name in commands {
            if let Err(e) = Command::parse(&name, Value::Object(serde_json::Map::new())) {
                assert!(!e.contains("unknown variant"), "{name}: {e}");
            }
        }
    }
}
