use serde::{Deserialize, Serialize};
use sm_domain::{AccountId, AttachmentId, DraftId, DraftKind, MessageId, Rev, ThreadId};
use ts_rs::TS;

use crate::Addr;

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
