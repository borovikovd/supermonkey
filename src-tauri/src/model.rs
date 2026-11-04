//! Every type that crosses the Tauri boundary. TypeScript copies are generated
//! into `src/bindings/` by ts-rs when `cargo test` runs.
//!
//! 64-bit integers are sent as JSON numbers. They are typed as `number` in
//! TypeScript because ids, revs, and millisecond times stay far below 2^53.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use crate::ids::{
    AccountId, AttachmentId, DraftId, MessageId, NoticeId, OutboxId, SplitId, SubId, ThreadId,
    UndoEntryId,
};

/// Milliseconds since the Unix epoch.
pub type Millis = i64;

// ---------------------------------------------------------------- views

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum View {
    Inbox,
    Done,
    /// Every thread outside Trash and Spam, as Gmail's All Mail.
    AllMail,
    Snoozed,
    Starred,
    Sent,
    Drafts,
    Trash,
    Spam,
    Label {
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum SubKey {
    /// The account list for the rail and global settings.
    App,
    /// One per account, always open.
    Account {
        account: AccountId,
    },
    List {
        account: AccountId,
        view: View,
    },
    Search {
        account: AccountId,
        query: String,
    },
    Conversation {
        thread: ThreadId,
        /// Opened from Trash or Spam: every non-draft message is listed.
        #[serde(rename = "trashMode")]
        trash_mode: bool,
        /// Open, as opposed to prefetched.
        open: bool,
    },
    Composer {
        draft: DraftId,
    },
}

// ---------------------------------------------------------------- common rows

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Addr {
    pub name: Option<String>,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SendAs {
    pub email: String,
    pub name: String,
    pub is_default: bool,
    pub is_primary: bool,
}

/// One row of a list. Threads use their id as `key`; the Drafts list uses draft ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListRow {
    pub key: String,
    pub thread: Option<ThreadId>,
    pub draft: Option<DraftId>,
    pub sender: String,
    pub sender_email: String,
    pub subject: String,
    pub snippet: String,
    #[ts(type = "number")]
    pub time: Millis,
    pub unread: bool,
    pub starred: bool,
    pub has_attachments: bool,
    pub message_count: u32,
    /// Label ids the thread is in, by the rule in SPEC Views (system and user).
    pub labels: Vec<String>,
    #[ts(type = "number | null")]
    pub snooze_until: Option<Millis>,
    pub split: Option<SplitId>,
    #[ts(type = "number")]
    pub rev: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListEntry {
    pub key: String,
    pub split: Option<SplitId>,
}

/// Insert or move `key` so it follows `after` (None = first).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListMove {
    pub key: String,
    pub split: Option<SplitId>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AttachmentInfo {
    pub id: AttachmentId,
    pub filename: String,
    pub mime_type: String,
    #[ts(type = "number")]
    pub size: i64,
    pub inline: bool,
    pub downloaded: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent flags the UI reads per message."
)]
pub struct MessageSummary {
    pub id: MessageId,
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub snippet: String,
    #[ts(type = "number")]
    pub date: Millis,
    pub unread: bool,
    pub starred: bool,
    /// Sent from one of this account's addresses.
    pub from_me: bool,
    pub in_trash: bool,
    pub in_spam: bool,
    pub has_body: bool,
    /// Plain-text mail; render with the app's own type.
    pub plain: bool,
    pub attachments: Vec<AttachmentInfo>,
    #[ts(type = "number")]
    pub rev: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MessageBody {
    pub id: MessageId,
    pub html: String,
    pub quote_html: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DraftKind {
    New,
    Reply,
    Forward,
}

impl DraftKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DraftKind::New => "new",
            DraftKind::Reply => "reply",
            DraftKind::Forward => "forward",
        }
    }
    pub fn parse(s: &str) -> DraftKind {
        match s {
            "reply" => DraftKind::Reply,
            "forward" => DraftKind::Forward,
            _ => DraftKind::New,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftAttachment {
    /// Stable id inside the draft.
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    #[ts(type = "number")]
    pub size: i64,
    /// Local file once available.
    pub path: Option<String>,
    /// Source attachment row when carried from a forwarded message or a Gmail draft.
    pub source: Option<AttachmentId>,
}

/// A collapsed draft shown under a thread.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftSummary {
    pub id: DraftId,
    pub kind: DraftKind,
    pub target: Option<MessageId>,
    pub started_on_latest: bool,
    pub to: Vec<Addr>,
    pub snippet: String,
    #[ts(type = "number")]
    pub updated_at: Millis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum OutboxState {
    Waiting,
    Sending,
    Sent,
}

impl OutboxState {
    pub fn parse(s: &str) -> OutboxState {
        match s {
            "sending" => OutboxState::Sending,
            "sent" => OutboxState::Sent,
            _ => OutboxState::Waiting,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OutboxRow {
    pub id: OutboxId,
    pub account: AccountId,
    pub draft: DraftId,
    pub thread: Option<ThreadId>,
    #[ts(type = "number")]
    pub send_after: Millis,
    pub state: OutboxState,
    pub sent_message_gmail_id: Option<String>,
    pub undo_requested: bool,
    pub toast_closed: bool,
    pub delayed: bool,
    /// For the pending message shown in the thread.
    pub to: Vec<Addr>,
    pub snippet: String,
    pub subject: String,
    /// The undo entry for this send, if it is still in the history.
    pub undo_entry: Option<UndoEntryId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConversationData {
    pub thread: ThreadId,
    pub account: AccountId,
    pub subject: String,
    pub labels: Vec<String>,
    #[ts(type = "number | null")]
    pub snooze_until: Option<Millis>,
    /// Oldest first, already filtered for the view (trash mode or not), no drafts.
    pub messages: Vec<MessageSummary>,
    pub drafts: Vec<DraftSummary>,
    pub outbox: Vec<OutboxRow>,
    /// The thread no longer exists locally.
    pub gone: bool,
    #[ts(type = "number")]
    pub rev: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftState {
    pub id: DraftId,
    pub account: AccountId,
    pub kind: DraftKind,
    pub reply_all: bool,
    pub thread: Option<ThreadId>,
    pub target: Option<MessageId>,
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub body_html: String,
    /// Read-only quote block shown collapsed under the editor.
    pub quote_html: Option<String>,
    pub attachments: Vec<DraftAttachment>,
    pub resumed: bool,
    pub gone: bool,
    #[ts(type = "number")]
    pub rev: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LabelInfo {
    pub id: String,
    pub name: String,
    pub system: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SplitKind {
    Custom,
    Important,
    Other,
}

impl SplitKind {
    pub fn parse(s: &str) -> SplitKind {
        match s {
            "important" => SplitKind::Important,
            "other" => SplitKind::Other,
            _ => SplitKind::Custom,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SplitInfo {
    pub id: SplitId,
    pub name: String,
    pub kind: SplitKind,
    pub query: String,
    pub enabled: bool,
    pub hide_when_empty: bool,
    pub position: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PinInfo {
    pub kind: PinKind,
    pub value: String,
    pub split: SplitId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PinKind {
    Sender,
    Domain,
    Label,
}

impl PinKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PinKind::Sender => "sender",
            PinKind::Domain => "domain",
            PinKind::Label => "label",
        }
    }
    pub fn parse(s: &str) -> PinKind {
        match s {
            "domain" => PinKind::Domain,
            "label" => PinKind::Label,
            _ => PinKind::Sender,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SyncStatus {
    Idle,
    Syncing,
    Offline,
    /// Retrying after rate limits or server errors for more than 30 s.
    Retrying,
    NeedsSignin,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountInfo {
    pub id: AccountId,
    pub email: String,
    pub name: String,
    pub hue: String,
    pub position: i32,
    pub inbox_unread: u32,
    pub needs_signin: bool,
    pub muted: bool,
    pub sync_status: SyncStatus,
    pub send_as: Vec<SendAs>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UndoEntryInfo {
    pub id: UndoEntryId,
    pub kind: UndoKind,
    pub label: String,
    #[ts(type = "number[]")]
    pub threads: Vec<ThreadId>,
    pub outbox: Option<OutboxId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum UndoKind {
    Mail,
    Send,
    Pin,
}

impl UndoKind {
    pub fn parse(s: &str) -> UndoKind {
        match s {
            "send" => UndoKind::Send,
            "pin" => UndoKind::Pin,
            _ => UndoKind::Mail,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NoticeKind {
    /// Gmail rejected a queued action; it was rolled back.
    Rejected,
    /// Gmail rejected a send; the draft goes back to a composer.
    SendFailed,
    /// A held send undo resolved as not sent; the draft goes back to a composer.
    SendUndone,
    /// The draft open in a composer was sent or deleted elsewhere.
    DraftGone,
    Info,
}

impl NoticeKind {
    pub fn parse(s: &str) -> NoticeKind {
        match s {
            "rejected" => NoticeKind::Rejected,
            "send_failed" => NoticeKind::SendFailed,
            "send_undone" => NoticeKind::SendUndone,
            "draft_gone" => NoticeKind::DraftGone,
            _ => NoticeKind::Info,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Notice {
    pub id: NoticeId,
    pub kind: NoticeKind,
    pub message: String,
    pub draft: Option<DraftId>,
    pub thread: Option<ThreadId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountSummary {
    pub account: AccountInfo,
    pub labels: Vec<LabelInfo>,
    pub splits: Vec<SplitInfo>,
    pub pins: Vec<PinInfo>,
    pub outbox: Vec<OutboxRow>,
    /// Newest last.
    pub undo: Vec<UndoEntryInfo>,
    pub notices: Vec<Notice>,
    pub queued_ops: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GlobalSettings {
    pub remote_images: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppState {
    pub accounts: Vec<AccountInfo>,
    pub settings: GlobalSettings,
    pub online: bool,
}

// ---------------------------------------------------------------- channel

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum SubUpdate {
    /// `ids` is set on a first load; patches carry `removed` and `moved` instead.
    /// `rows` holds the full row of every changed id in the list (and a window on first load).
    #[serde(rename_all = "camelCase")]
    List {
        sub: SubId,
        first: bool,
        ids: Option<Vec<ListEntry>>,
        removed: Vec<String>,
        moved: Vec<ListMove>,
        rows: Vec<ListRow>,
        /// Search only: a Gmail query is still running.
        searching: bool,
        /// Gmail has another page for this list.
        more: bool,
    },
    Conversation {
        sub: SubId,
        first: bool,
        data: ConversationData,
    },
    Composer {
        sub: SubId,
        first: bool,
        draft: DraftState,
    },
    Account {
        sub: SubId,
        first: bool,
        summary: AccountSummary,
    },
    App {
        sub: SubId,
        first: bool,
        state: AppState,
    },
}

/// One message per writer commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ChannelMessage {
    #[ts(type = "number")]
    pub rev: i64,
    /// Highest UI action number applied in this snapshot.
    #[ts(type = "number")]
    pub ack: i64,
    pub updates: Vec<SubUpdate>,
}

// ---------------------------------------------------------------- actions

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "at", rename_all = "camelCase")]
#[ts(export)]
pub enum SnoozeChange {
    Set(#[ts(type = "number")] Millis),
    Clear,
}

/// What an action is called in the undo toast and history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ModifyKind {
    Done,
    ReturnToInbox,
    Snooze,
    Trash,
    Spam,
    Star,
    Unstar,
    Read,
    Unread,
    Label,
    RemoveLabel,
}

impl ModifyKind {
    pub fn label(self) -> &'static str {
        match self {
            ModifyKind::Done => "Marked done",
            ModifyKind::ReturnToInbox => "Moved to inbox",
            ModifyKind::Snooze => "Snoozed",
            ModifyKind::Trash => "Moved to trash",
            ModifyKind::Spam => "Marked as spam",
            ModifyKind::Star => "Starred",
            ModifyKind::Unstar => "Unstarred",
            ModifyKind::Read => "Marked read",
            ModifyKind::Unread => "Marked unread",
            ModifyKind::Label => "Labeled",
            ModifyKind::RemoveLabel => "Label removed",
        }
    }
}

/// A resolved mail change, never a toggle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModifyAction {
    pub account: AccountId,
    pub kind: ModifyKind,
    #[ts(type = "number[]")]
    pub threads: Vec<ThreadId>,
    pub add: Vec<String>,
    pub remove: Vec<String>,
    pub snooze: Option<SnoozeChange>,
    /// Read-on-open is not an undo entry.
    pub undoable: bool,
}

/// The composer's full state, sent with saves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftFields {
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    pub body_html: String,
    pub attachments: Vec<DraftAttachment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum DraftSave {
    /// A fresh composer's first save. Rust fills From, To, Cc, Subject, and the quote.
    #[serde(rename_all = "camelCase")]
    Create {
        account: AccountId,
        kind: DraftKind,
        reply_all: bool,
        thread: Option<ThreadId>,
        target: Option<MessageId>,
        /// The composer was aimed at the thread's latest message.
        started_on_latest: bool,
    },
    Update {
        fields: DraftFields,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Action {
    /// First call after a page load: closes every subscription of the previous page.
    Reset,
    Subscribe {
        sub: SubId,
        key: SubKey,
    },
    Unsubscribe {
        sub: SubId,
    },
    Modify(ModifyAction),
    Undo {
        account: AccountId,
        entry: UndoEntryId,
    },
    /// Create a label (with a UI-made local id) and optionally apply it in the same action.
    CreateLabel {
        account: AccountId,
        id: String,
        name: String,
        apply: Option<ModifyAction>,
    },
    SaveDraft {
        draft: DraftId,
        save: DraftSave,
    },
    CloseComposer {
        draft: DraftId,
    },
    SendDraft {
        draft: DraftId,
    },
    DiscardDraft {
        draft: DraftId,
    },
    /// The user closed a send toast.
    CloseToast {
        outbox: OutboxId,
    },
    AckNotice {
        notice: NoticeId,
    },
    SetSetting {
        account: Option<AccountId>,
        key: String,
        value: String,
    },
    Pin {
        account: AccountId,
        kind: PinKind,
        value: String,
        split: SplitId,
    },
    SaveSplits {
        account: AccountId,
        splits: Vec<SplitInfo>,
        /// Ids of custom splits to delete.
        #[ts(type = "number[]")]
        deleted: Vec<SplitId>,
    },
    RemoveAccount {
        account: AccountId,
    },
    /// Fetch older search pages / view pages from Gmail.
    MoreResults {
        sub: SubId,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NumberedAction {
    #[ts(type = "number")]
    pub n: i64,
    pub action: Action,
}

// ---------------------------------------------------------------- commands

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SnoozeChoice {
    pub label: String,
    #[ts(type = "number")]
    pub at: Millis,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaunchInfo {
    /// Continue numbering actions after this.
    #[ts(type = "number")]
    pub last_action: i64,
    pub debug: bool,
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
    List { senders: Vec<BlockedSender> },
    NeedsPermission,
}
