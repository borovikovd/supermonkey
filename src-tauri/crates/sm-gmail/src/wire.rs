use serde::{Deserialize, Deserializer};
use serde_json::Value;
use sm_domain::{GmailDraftId, GmailMessageId, GmailThreadId, LabelId, Millis};

use crate::ids::{FilterId, HistoryId, PageToken, string_u64};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub email_address: String,
    pub history_id: HistoryId,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendAs {
    #[serde(rename = "sendAsEmail")]
    pub email: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub is_primary: bool,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LabelType {
    System,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub id: LabelId,
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: Option<LabelType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: GmailMessageId,
    pub thread_id: GmailThreadId,
    #[serde(default)]
    pub label_ids: Vec<LabelId>,
    #[serde(default)]
    pub snippet: String,
    #[serde(default)]
    pub history_id: Option<HistoryId>,
    #[serde(default, deserialize_with = "opt_millis")]
    pub internal_date: Option<Millis>,
    #[serde(default)]
    pub payload: Option<Value>,
    #[serde(default)]
    pub raw: Option<String>,
}

impl Message {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.payload.as_ref()?["headers"]
            .as_array()?
            .iter()
            .find(|h| {
                h["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })?["value"]
            .as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: GmailThreadId,
    #[serde(default)]
    pub snippet: String,
    #[serde(default)]
    pub history_id: Option<HistoryId>,
    #[serde(default)]
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPage {
    #[serde(default)]
    pub threads: Vec<Thread>,
    #[serde(default)]
    pub next_page_token: Option<PageToken>,
    #[serde(default)]
    pub result_size_estimate: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HistoryMessage {
    pub message: Message,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryLabelChange {
    pub message: Message,
    #[serde(default)]
    pub label_ids: Vec<LabelId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub id: HistoryId,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub messages_added: Vec<HistoryMessage>,
    #[serde(default)]
    pub messages_deleted: Vec<HistoryMessage>,
    #[serde(default)]
    pub labels_added: Vec<HistoryLabelChange>,
    #[serde(default)]
    pub labels_removed: Vec<HistoryLabelChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
    #[serde(default)]
    pub next_page_token: Option<PageToken>,
    pub history_id: HistoryId,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Draft {
    pub id: GmailDraftId,
    pub message: Message,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftPage {
    #[serde(default)]
    pub drafts: Vec<Draft>,
    #[serde(default)]
    pub next_page_token: Option<PageToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Filter {
    pub id: FilterId,
    #[serde(default)]
    pub criteria: FilterCriteria,
    #[serde(default)]
    pub action: FilterAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct FilterCriteria {
    #[serde(default)]
    pub from: Option<String>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterAction {
    #[serde(default)]
    pub add_label_ids: Vec<LabelId>,
    #[serde(default)]
    pub remove_label_ids: Vec<LabelId>,
    #[serde(flatten)]
    pub other: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Minimal,
    Metadata,
    Full,
    Raw,
}

impl Format {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Metadata => "metadata",
            Self::Full => "full",
            Self::Raw => "raw",
        }
    }

    pub(crate) const fn is_full(self) -> bool {
        matches!(self, Self::Full | Self::Raw)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThreadQuery<'a> {
    pub q: Option<&'a str>,
    pub label_ids: &'a [LabelId],
    pub page_token: Option<&'a PageToken>,
    pub max_results: Option<u32>,
    pub include_spam_trash: bool,
}

#[derive(Deserialize)]
pub(crate) struct PartBody {
    #[serde(default)]
    pub(crate) data: Option<String>,
}

fn opt_millis<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Millis>, D::Error> {
    use serde::de::Error as _;

    #[derive(Deserialize)]
    struct Raw(#[serde(deserialize_with = "string_u64")] u64);
    Option::<Raw>::deserialize(d)?
        .map(|Raw(ms)| i64::try_from(ms).map(Millis).map_err(D::Error::custom))
        .transpose()
}
