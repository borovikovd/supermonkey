//! The Gmail REST resources the app reads, as Gmail sends them.
//!
//! Field names follow Gmail's JSON. Ids are `sm-domain`'s newtypes, which are
//! serde-transparent. Gmail encodes 64-bit numbers (`historyId`,
//! `internalDate`) as strings; these types read them as numbers. A message's
//! `payload` stays raw JSON, for `sm-mime` to parse.

use serde::{Deserialize, Deserializer};
use serde_json::Value;
use sm_domain::{GmailDraftId, GmailMessageId, GmailThreadId, LabelId, Millis};

use crate::ids::{FilterId, HistoryId, PageToken, string_u64};

/// `users.getProfile`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    /// The account's address.
    pub email_address: String,
    /// The mailbox's current history cursor.
    pub history_id: HistoryId,
}

/// One entry of `users.settings.sendAs.list`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendAs {
    /// The address mail is sent from.
    pub send_as_email: String,
    /// The display name, empty when unset.
    #[serde(default)]
    pub display_name: String,
    /// The account's own address.
    #[serde(default)]
    pub is_primary: bool,
    /// The default `From` for new mail.
    #[serde(default)]
    pub is_default: bool,
}

/// Whether a label is Gmail's own or one the user made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LabelType {
    /// `INBOX`, `STARRED`, `CATEGORY_*`, and the rest of Gmail's.
    System,
    /// Made by the user.
    User,
}

/// A Gmail label.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    /// Gmail's id, e.g. `INBOX` or `Label_12`.
    pub id: LabelId,
    /// The name, with `/` between nesting levels.
    pub name: String,
    /// Gmail's or the user's.
    #[serde(rename = "type", default)]
    pub label_type: Option<LabelType>,
}

/// A Gmail message. Which fields are set depends on the [`Format`] asked for
/// and where the message appears: history records carry only ids and labels.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Gmail's id of the message.
    pub id: GmailMessageId,
    /// Gmail's id of its thread.
    pub thread_id: GmailThreadId,
    /// Its labels.
    #[serde(default)]
    pub label_ids: Vec<LabelId>,
    /// Gmail's plain-text preview.
    #[serde(default)]
    pub snippet: String,
    /// The history cursor of its last change.
    #[serde(default)]
    pub history_id: Option<HistoryId>,
    /// When Gmail received it.
    #[serde(default, deserialize_with = "opt_millis")]
    pub internal_date: Option<Millis>,
    /// The MIME tree (`full`) or the headers (`metadata`), for `sm-mime`.
    #[serde(default)]
    pub payload: Option<Value>,
    /// The RFC 822 bytes, base64url (`raw`).
    #[serde(default)]
    pub raw: Option<String>,
}

impl Message {
    /// The first value of a top-level header, by case-insensitive name.
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

/// A Gmail thread. In `threads.list` only `id`, `snippet`, and `history_id`
/// are set.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    /// Gmail's id of the thread.
    pub id: GmailThreadId,
    /// Gmail's preview of its latest message.
    #[serde(default)]
    pub snippet: String,
    /// The history cursor of its last change.
    #[serde(default)]
    pub history_id: Option<HistoryId>,
    /// Its messages, oldest first.
    #[serde(default)]
    pub messages: Vec<Message>,
}

/// One page of `threads.list`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPage {
    /// The threads, newest first.
    #[serde(default)]
    pub threads: Vec<Thread>,
    /// The next page, if any.
    #[serde(default)]
    pub next_page_token: Option<PageToken>,
    /// Gmail's guess at the total.
    #[serde(default)]
    pub result_size_estimate: u64,
}

/// A message named in a history record.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HistoryMessage {
    /// Ids and labels only.
    pub message: Message,
}

/// A label change in a history record.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryLabelChange {
    /// The message, ids and labels only.
    pub message: Message,
    /// The labels added or removed.
    #[serde(default)]
    pub label_ids: Vec<LabelId>,
}

/// One record of `history.list`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRecord {
    /// The record's cursor.
    pub id: HistoryId,
    /// Every message the record touches.
    #[serde(default)]
    pub messages: Vec<Message>,
    /// Messages added.
    #[serde(default)]
    pub messages_added: Vec<HistoryMessage>,
    /// Messages deleted for good.
    #[serde(default)]
    pub messages_deleted: Vec<HistoryMessage>,
    /// Labels added to messages.
    #[serde(default)]
    pub labels_added: Vec<HistoryLabelChange>,
    /// Labels removed from messages.
    #[serde(default)]
    pub labels_removed: Vec<HistoryLabelChange>,
}

/// One page of `history.list`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    /// The records, oldest first.
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
    /// The next page, if any.
    #[serde(default)]
    pub next_page_token: Option<PageToken>,
    /// The mailbox's current cursor.
    pub history_id: HistoryId,
}

/// A Gmail draft and its current message.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Draft {
    /// Gmail's id of the draft, stable across updates.
    pub id: GmailDraftId,
    /// Its message, whose id changes on every update. In `drafts.list` only
    /// `id` and `thread_id` are set.
    pub message: Message,
}

/// One page of `drafts.list`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftPage {
    /// The drafts, newest first.
    #[serde(default)]
    pub drafts: Vec<Draft>,
    /// The next page, if any.
    #[serde(default)]
    pub next_page_token: Option<PageToken>,
}

/// A Gmail settings filter.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Filter {
    /// Gmail's id of the filter.
    pub id: FilterId,
    /// What it matches.
    #[serde(default)]
    pub criteria: FilterCriteria,
    /// What it does.
    #[serde(default)]
    pub action: FilterAction,
}

/// What a filter matches. Criteria other than `from` stay in `other`, so a
/// filter that matches more than a sender can be told apart.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct FilterCriteria {
    /// An address, `@domain`, or any Gmail `from:` term.
    #[serde(default)]
    pub from: Option<String>,
    /// Every other criterion.
    #[serde(flatten)]
    pub other: serde_json::Map<String, Value>,
}

/// What a filter does. Actions other than label changes stay in `other`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterAction {
    /// Labels it adds.
    #[serde(default)]
    pub add_label_ids: Vec<LabelId>,
    /// Labels it removes.
    #[serde(default)]
    pub remove_label_ids: Vec<LabelId>,
    /// Every other action, such as `forward`.
    #[serde(flatten)]
    pub other: serde_json::Map<String, Value>,
}

/// How much of a message, draft, or thread to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// Ids and labels.
    Minimal,
    /// Headers, no bodies.
    Metadata,
    /// The MIME tree with bodies.
    Full,
    /// The RFC 822 bytes. Messages and drafts only.
    Raw,
}

impl Format {
    /// The `format` query value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Metadata => "metadata",
            Self::Full => "full",
            Self::Raw => "raw",
        }
    }

    /// Whether Gmail charges this format as a full read.
    pub(crate) fn is_full(self) -> bool {
        matches!(self, Self::Full | Self::Raw)
    }
}

/// The parameters of `threads.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadQuery {
    /// A Gmail search query.
    pub q: Option<String>,
    /// Threads must carry every one of these labels.
    pub label_ids: Vec<LabelId>,
    /// The page to read; `None` for the first.
    pub page_token: Option<PageToken>,
    /// Gmail's default is 100, its maximum 500.
    pub max_results: Option<u32>,
    /// Include Spam and Trash.
    pub include_spam_trash: bool,
}

/// A part body, as `messages.attachments.get` answers.
#[derive(Deserialize)]
pub(crate) struct PartBody {
    /// Base64url bytes.
    #[serde(default)]
    pub(crate) data: Option<String>,
}

/// Reads `internalDate`: Unix milliseconds as a string.
fn opt_millis<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Millis>, D::Error> {
    use serde::de::Error as _;

    #[derive(Deserialize)]
    struct Raw(#[serde(deserialize_with = "string_u64")] u64);
    Option::<Raw>::deserialize(d)?
        .map(|Raw(ms)| i64::try_from(ms).map(Millis).map_err(D::Error::custom))
        .transpose()
}
