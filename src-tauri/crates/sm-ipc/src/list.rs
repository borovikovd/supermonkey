use serde::{Deserialize, Serialize};
use sm_domain::{DraftId, LabelId, Millis, Rev, SplitId, ThreadId};
use ts_rs::TS;

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
