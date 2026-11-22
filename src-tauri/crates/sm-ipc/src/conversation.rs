use serde::{Deserialize, Serialize};
use sm_domain::{
    AccountId, AttachmentId, DraftId, DraftKind, GmailMessageId, LabelId, MessageId, Millis,
    OutboxId, OutboxState, Rev, ThreadId, UndoEntryId,
};
use ts_rs::TS;

use crate::Addr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConversationData {
    pub thread: ThreadId,
    pub account: AccountId,
    pub subject: String,
    pub labels: Vec<LabelId>,
    pub snooze_until: Option<Millis>,
    pub messages: Vec<MessageSummary>,
    pub drafts: Vec<DraftSummary>,
    pub outbox: Vec<OutboxRow>,
    pub gone: bool,
    pub rev: Rev,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
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
    pub date: Millis,
    pub unread: bool,
    pub starred: bool,
    pub in_trash: bool,
    pub in_spam: bool,
    pub has_body: bool,
    pub plain: bool,
    pub attachments: Vec<AttachmentInfo>,
    pub rev: Rev,
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
    pub downloaded: bool,
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
    pub send_after: Millis,
    pub state: OutboxState,
    pub sent_message_gmail_id: Option<GmailMessageId>,
    pub undo_requested: bool,
    pub toast_closed: bool,
    pub delayed: bool,
    pub to: Vec<Addr>,
    pub snippet: String,
    pub subject: String,
    pub undo_entry: Option<UndoEntryId>,
}
