//! Serde types for the Gmail REST resources the app uses.
//!
//! Gmail encodes 64-bit ids (`historyId`, `internalDate`) as JSON strings; these
//! types expose them as integers. Message payloads stay raw JSON: `ub-mime` parses them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `users.getProfile`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub email_address: String,
    #[serde(default)]
    pub messages_total: u64,
    #[serde(default)]
    pub threads_total: u64,
    #[serde(with = "string_u64")]
    pub history_id: u64,
}

/// One entry of `users.settings.sendAs.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendAs {
    pub send_as_email: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_address: Option<String>,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub is_primary: bool,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub treat_as_alias: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_status: Option<String>,
}

/// Whether a label is one of Gmail's own or one the user made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LabelType {
    System,
    User,
}

/// A Gmail label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub id: String,
    pub name: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub label_type: Option<LabelType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_list_visibility: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_list_visibility: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages_total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages_unread: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads_total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads_unread: Option<u64>,
}

/// A Gmail message. Which fields are present depends on the requested [`Format`]
/// and on where the message appears (history records carry only ids and labels).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
    #[serde(default)]
    pub snippet: String,
    #[serde(
        default,
        with = "opt_string_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub history_id: Option<u64>,
    /// Milliseconds since the Unix epoch.
    #[serde(
        default,
        with = "opt_string_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub internal_date: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_estimate: Option<u64>,
    /// The MIME tree (`format=full`) or headers only (`format=metadata`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    /// Base64url RFC 822 bytes (`format=raw`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

/// A Gmail thread. In `threads.list` results only `id`, `snippet` and `history_id` are set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: String,
    #[serde(default)]
    pub snippet: String,
    #[serde(
        default,
        with = "opt_string_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub history_id: Option<u64>,
    #[serde(default)]
    pub messages: Vec<Message>,
}

/// `threads.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListThreadsResponse {
    #[serde(default)]
    pub threads: Vec<Thread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
    #[serde(default)]
    pub result_size_estimate: u64,
}

/// A message reference inside a history record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub message: Message,
}

/// A label change inside a history record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryLabelChange {
    pub message: Message,
    #[serde(default)]
    pub label_ids: Vec<String>,
}

/// One record of `history.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct History {
    #[serde(with = "string_u64")]
    pub id: u64,
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

/// `history.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListHistoryResponse {
    #[serde(default)]
    pub history: Vec<History>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
    /// The mailbox's current history id.
    #[serde(with = "string_u64")]
    pub history_id: u64,
}

/// A Gmail draft and its current message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub id: String,
    pub message: Message,
}

/// `drafts.list`. Each draft's message has only `id` and `thread_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListDraftsResponse {
    #[serde(default)]
    pub drafts: Vec<Draft>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
    #[serde(default)]
    pub result_size_estimate: u64,
}

/// A part body, as returned by `messages.attachments.get`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessagePartBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<String>,
    #[serde(default)]
    pub size: u64,
    /// Base64url bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// How much of a message or thread to fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Ids and labels only.
    Minimal,
    /// Headers, no bodies.
    Metadata,
    /// The full MIME tree with bodies.
    Full,
    /// The RFC 822 bytes. Messages and drafts only.
    Raw,
}

impl Format {
    /// The `format` query value.
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Minimal => "minimal",
            Format::Metadata => "metadata",
            Format::Full => "full",
            Format::Raw => "raw",
        }
    }
}

mod string_u64 {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    #[allow(clippy::trivially_copy_pass_by_ref)] // serde's serialize_with signature.
    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::String(s) => s.parse().map_err(D::Error::custom),
            serde_json::Value::Number(n) => n.as_u64().ok_or_else(|| D::Error::custom("not a u64")),
            other => Err(D::Error::custom(format!("expected a u64, got {other}"))),
        }
    }
}

mod opt_string_u64 {
    use serde::{Deserialize, Deserializer, Serializer};

    #[allow(clippy::ref_option)] // serde's serialize_with signature.
    pub fn serialize<S: Serializer>(v: &Option<u64>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.serialize_str(&v.to_string()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
        #[derive(Deserialize)]
        struct Wrapper(#[serde(with = "super::string_u64")] u64);
        Ok(Option::<Wrapper>::deserialize(d)?.map(|w| w.0))
    }
}
