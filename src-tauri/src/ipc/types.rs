use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::domain::{
    AccountId, Addr, AttachmentId, DraftId, DraftKind, LabelId, MessageId, Millis, NoticeId,
    NoticeKind, OutboxId, OutboxState, Rev, SplitId, SplitKind, SyncStatus, ThreadId, UndoEntryId,
};

const AVATAR_HUES: [&str; 6] = [
    "#714cb6", "#d6478a", "#2e8b57", "#d97706", "#2563eb", "#0891b2",
];

#[must_use]
pub fn avatar_hue(slot: i64) -> &'static str {
    let idx = usize::try_from(slot.rem_euclid(6)).unwrap_or(0);
    AVATAR_HUES.get(idx).copied().unwrap_or(AVATAR_HUES[0])
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppState {
    pub accounts: Vec<AccountInfo>,
    pub settings: GlobalSettings,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GlobalSettings {
    pub remote_images: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountInfo {
    pub id: AccountId,
    pub email: String,
    pub name: String,
    pub hue: String,
    pub inbox_unread: u32,
    pub needs_signin: bool,
    pub muted: bool,
    pub sync_status: SyncStatus,
    pub send_as: Vec<SendAs>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SendAs {
    pub email: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountSummary {
    pub account: AccountInfo,
    pub labels: Vec<LabelInfo>,
    pub splits: Vec<SplitInfo>,
    pub outbox: Vec<OutboxRow>,
    pub undo: Vec<UndoEntryInfo>,
    pub notices: Vec<Notice>,
    pub queued_ops: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LabelInfo {
    pub id: LabelId,
    pub name: String,
    pub system: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SplitInfo {
    pub id: SplitId,
    pub name: String,
    pub kind: SplitKind,
    pub query: String,
    pub enabled: bool,
    pub hide_when_empty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UndoEntryInfo {
    pub id: UndoEntryId,
    pub outbox: Option<OutboxId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Notice {
    pub id: NoticeId,
    pub kind: NoticeKind,
    pub message: String,
    pub draft: Option<DraftId>,
    pub thread: Option<ThreadId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
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
    pub time: Millis,
    pub unread: bool,
    pub starred: bool,
    pub has_attachments: bool,
    pub message_count: u32,
    pub labels: Vec<LabelId>,
    pub snooze_until: Option<Millis>,
    pub split: Option<SplitId>,
    pub rev: Rev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListEntry {
    pub key: String,
    pub split: Option<SplitId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListMove {
    pub key: String,
    pub split: Option<SplitId>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConversationData {
    pub thread: ThreadId,
    pub subject: String,
    pub messages: Vec<MessageSummary>,
    pub drafts: Vec<DraftSummary>,
    pub outbox: Vec<OutboxRow>,
    pub gone: bool,
    pub rev: Rev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MessageSummary {
    pub id: MessageId,
    pub from: MessageAddr,
    pub to: Vec<MessageAddr>,
    pub cc: Vec<MessageAddr>,
    pub bcc: Vec<MessageAddr>,
    pub subject: String,
    pub snippet: String,
    pub date: Millis,
    pub unread: bool,
    pub starred: bool,
    pub has_body: bool,
    pub attachments: Vec<AttachmentInfo>,
    pub rev: Rev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MessageAddr {
    pub name: Option<String>,
    pub email: String,
    pub me: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MessageBody {
    pub id: MessageId,
    pub html: String,
    pub quote_html: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AttachmentInfo {
    pub id: AttachmentId,
    pub filename: String,
    pub mime_type: String,
    #[ts(type = "number")]
    pub size: i64,
    pub inline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftSummary {
    pub id: DraftId,
    pub kind: DraftKind,
    pub target: Option<MessageId>,
    pub started_on_latest: bool,
    pub to: Vec<Addr>,
    pub snippet: String,
    pub updated_at: Millis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OutboxRow {
    pub id: OutboxId,
    pub account: AccountId,
    pub draft: DraftId,
    pub thread: Option<ThreadId>,
    pub state: OutboxState,
    pub undo_requested: bool,
    pub toast_closed: bool,
    pub delayed: bool,
    pub to: Vec<Addr>,
    pub snippet: String,
    pub subject: String,
    pub undo_entry: Option<UndoEntryId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
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
    pub quote_html: Option<String>,
    pub attachments: Vec<DraftAttachment>,
    pub gone: bool,
    pub rev: Rev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum DraftSave {
    #[serde(rename_all = "camelCase")]
    Create {
        account: AccountId,
        kind: DraftKind,
        reply_all: bool,
        thread: Option<ThreadId>,
        target: Option<MessageId>,
        started_on_latest: bool,
    },
    Update {
        fields: DraftFields,
        rev: Rev,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DraftAttachment {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    #[ts(type = "number")]
    pub size: i64,
    pub path: Option<String>,
    pub source: Option<AttachmentId>,
}
