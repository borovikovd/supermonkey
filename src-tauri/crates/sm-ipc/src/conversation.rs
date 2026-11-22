use serde::{Deserialize, Serialize};
use sm_domain::{
    AccountId, AttachmentId, DraftId, DraftKind, MessageId, Millis, OutboxId, OutboxState, Rev,
    ThreadId, UndoEntryId,
};
use ts_rs::TS;

use crate::Addr;

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
