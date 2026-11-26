use serde::{Deserialize, Deserializer, de};

use crate::domain::{
    FilterId, GmailDraftId, GmailMessageId, GmailThreadId, HistoryId, LabelId, Millis, PageToken,
};
use crate::mime::WirePart;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Full,
    Metadata,
    Minimal,
}

impl Format {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Metadata => "metadata",
            Self::Minimal => "minimal",
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ThreadQuery<'a> {
    pub q: Option<&'a str>,
    pub label_id: Option<&'a LabelId>,
    pub include_spam_trash: bool,
    pub max_results: Option<u32>,
    pub page_token: Option<&'a PageToken>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub email_address: String,
    pub history_id: HistoryId,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendAs {
    #[serde(rename = "sendAsEmail")]
    pub email: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub is_primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LabelType {
    System,
    User,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Label {
    pub id: LabelId,
    pub name: String,
    #[serde(rename = "type", default = "default_label_type")]
    pub kind: LabelType,
}

const fn default_label_type() -> LabelType {
    LabelType::User
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: GmailMessageId,
    #[serde(default)]
    pub thread_id: Option<GmailThreadId>,
    #[serde(default)]
    pub label_ids: Vec<LabelId>,
    #[serde(default)]
    pub snippet: String,
    #[serde(default)]
    pub history_id: Option<HistoryId>,
    #[serde(default, deserialize_with = "opt_millis")]
    pub internal_date: Option<Millis>,
    #[serde(default)]
    pub payload: Option<WirePart>,
}

impl Message {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.payload
            .as_ref()?
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: GmailThreadId,
    #[serde(default)]
    pub history_id: Option<HistoryId>,
    #[serde(default)]
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRef {
    pub id: GmailThreadId,
    #[serde(default)]
    pub history_id: Option<HistoryId>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPage {
    #[serde(default)]
    pub threads: Vec<ThreadRef>,
    #[serde(default)]
    pub next_page_token: Option<PageToken>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryMessageRef {
    pub message: Message,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    pub id: HistoryId,
    #[serde(default)]
    pub messages_added: Vec<HistoryMessageRef>,
    #[serde(default)]
    pub messages_deleted: Vec<HistoryMessageRef>,
    #[serde(default)]
    pub labels_added: Vec<HistoryMessageRef>,
    #[serde(default)]
    pub labels_removed: Vec<HistoryMessageRef>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
    #[serde(default)]
    pub history_id: Option<HistoryId>,
    #[serde(default)]
    pub next_page_token: Option<PageToken>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Draft {
    pub id: GmailDraftId,
    #[serde(default)]
    pub message: Option<Message>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftPage {
    #[serde(default)]
    pub drafts: Vec<Draft>,
    #[serde(default)]
    pub next_page_token: Option<PageToken>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Filter {
    pub id: FilterId,
    #[serde(default)]
    pub criteria: FilterCriteria,
    #[serde(default)]
    pub action: FilterAction,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct FilterCriteria {
    #[serde(default)]
    pub from: Option<String>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterAction {
    #[serde(default)]
    pub add_label_ids: Vec<LabelId>,
    #[serde(default)]
    pub remove_label_ids: Vec<LabelId>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct SendAsList {
    #[serde(default, rename = "sendAs")]
    pub(super) send_as: Vec<SendAs>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct LabelList {
    #[serde(default)]
    pub(super) labels: Vec<Label>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct FilterList {
    #[serde(default)]
    pub(super) filter: Vec<Filter>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct PartBody {
    #[serde(default)]
    pub(super) data: Option<String>,
}

fn opt_millis<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Millis>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Int(i64),
        Text(String),
    }
    let Some(raw) = Option::<Raw>::deserialize(deserializer)? else {
        return Ok(None);
    };
    match raw {
        Raw::Int(n) => Ok(Some(Millis(n))),
        Raw::Text(s) => s.parse::<i64>().map(|n| Some(Millis(n))).map_err(|_parse| {
            de::Error::invalid_value(de::Unexpected::Str(&s), &"an i64 internalDate")
        }),
    }
}
