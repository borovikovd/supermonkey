//! Parsing the `payload` of a Gmail `users.messages` resource.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::address::{Addr, parse_address_list};
use crate::collapse_whitespace;
use crate::encoding::{decode_base64url, decode_charset, decode_encoded_words, header_param};

/// An attachment or inline part of a received message.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PartInfo {
    /// Gmail's `partId`, e.g. `"1.2"`.
    pub part_id: String,
    /// Decoded filename. Empty when the part has none (calendar parts get `invite.ics`,
    /// attached messages `message.eml`).
    pub filename: String,
    /// Lowercased MIME type, e.g. `image/png`.
    pub mime_type: String,
    /// `body.size` as reported by Gmail (decoded bytes).
    pub size: u64,
    /// Present when the data must be fetched with `messages.attachments.get`.
    pub attachment_id: Option<String>,
    /// Content-ID without the angle brackets.
    pub content_id: Option<String>,
    /// `Content-Disposition: inline`, or no disposition but a Content-ID.
    pub inline: bool,
    /// The decoded bytes when Gmail inlined them in `body.data`.
    pub data: Option<Vec<u8>>,
}

/// The fields of a received message the app stores.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParsedMessage {
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub reply_to: Vec<Addr>,
    /// Decoded subject, trimmed. Empty when missing.
    pub subject: String,
    /// The `Date` header in Unix milliseconds, when it parses.
    pub date_ms: Option<i64>,
    /// Message ids are stored without angle brackets, here and in the fields below.
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// `List-Id` or `List-Unsubscribe` is present.
    pub is_list: bool,
    /// The `X-Ubermensch-Draft` header: the local draft id of a draft this app wrote.
    pub ub_draft_id: Option<String>,
    /// Decoded `text/html` body. Several body parts (Apple Mail splits around attachments)
    /// are joined in order.
    pub html: Option<String>,
    /// Decoded `text/plain` body, joined the same way.
    pub text: Option<String>,
    /// Attachments and inline parts, in document order. Body parts are not listed.
    pub parts: Vec<PartInfo>,
}

/// Parses the `payload` object of a Gmail `users.messages` resource (`format=full` or
/// `format=metadata`; metadata has headers only, so bodies and parts stay empty).
/// Bodies are base64url in `body.data`, decoded with the part's charset. A body part that
/// Gmail moved behind an `attachmentId` (very large bodies) is skipped.
pub fn parse_gmail_payload(payload: &Value) -> ParsedMessage {
    let header = |name: &str| find_header(payload, name);
    let addrs = |name: &str| header(name).map(parse_address_list).unwrap_or_default();

    let mut msg = ParsedMessage {
        from: addrs("From").into_iter().next(),
        to: addrs("To"),
        cc: addrs("Cc"),
        bcc: addrs("Bcc"),
        reply_to: addrs("Reply-To"),
        subject: header("Subject")
            .map(|s| decode_encoded_words(s).trim().to_string())
            .unwrap_or_default(),
        date_ms: header("Date").and_then(parse_date),
        message_id: header("Message-ID").and_then(|v| message_ids(v).into_iter().next()),
        in_reply_to: header("In-Reply-To").and_then(|v| message_ids(v).into_iter().next()),
        references: header("References").map(message_ids).unwrap_or_default(),
        is_list: header("List-Id").is_some() || header("List-Unsubscribe").is_some(),
        ub_draft_id: header("X-Ubermensch-Draft")
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty()),
        ..Default::default()
    };

    let mut found = Collected::default();
    found.visit(payload);
    msg.html = join_bodies(&found.html);
    msg.text = join_bodies(&found.text);
    msg.parts = found.parts;
    msg
}

fn join_bodies(bodies: &[String]) -> Option<String> {
    (!bodies.is_empty()).then(|| bodies.join("\n"))
}

/// What a walk of the part tree collects, in document order.
#[derive(Default)]
struct Collected {
    html: Vec<String>,
    text: Vec<String>,
    parts: Vec<PartInfo>,
}

impl Collected {
    fn visit(&mut self, part: &Value) {
        let mime_type = part["mimeType"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if mime_type.starts_with("multipart/") {
            for child in part["parts"].as_array().into_iter().flatten() {
                self.visit(child);
            }
            return;
        }

        let content_type = find_header(part, "Content-Type").unwrap_or("");
        let disposition = find_header(part, "Content-Disposition").unwrap_or("");
        let disposition_kind = disposition
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let mut filename = part["filename"]
            .as_str()
            .filter(|f| !f.is_empty())
            .map(str::to_string)
            .or_else(|| header_param(disposition, "filename"))
            .or_else(|| header_param(content_type, "name"))
            .unwrap_or_default();
        let body = &part["body"];
        let data = body["data"].as_str().and_then(decode_base64url);

        let is_body = (mime_type == "text/html" || mime_type == "text/plain")
            && filename.is_empty()
            && disposition_kind != "attachment";
        if is_body {
            if let Some(bytes) = data {
                let charset = header_param(content_type, "charset");
                let bodies = if mime_type == "text/html" {
                    &mut self.html
                } else {
                    &mut self.text
                };
                bodies.push(decode_charset(&bytes, charset.as_deref()));
            }
            return;
        }

        let attachment_id = body["attachmentId"].as_str().map(str::to_string);
        if data.is_none() && attachment_id.is_none() {
            return;
        }
        let content_id = find_header(part, "Content-ID")
            .map(bare_id)
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        if filename.is_empty() {
            filename = default_filename(&mime_type).to_string();
        }
        self.parts.push(PartInfo {
            part_id: part["partId"].as_str().unwrap_or("").to_string(),
            filename,
            inline: match disposition_kind.as_str() {
                "inline" => true,
                "" => content_id.is_some(),
                _ => false,
            },
            content_id,
            mime_type,
            size: body["size"].as_u64().unwrap_or(0),
            attachment_id,
            data,
        });
    }
}

/// The name a nameless part is saved under; empty when its type has no usual name.
fn default_filename(mime_type: &str) -> &'static str {
    match mime_type {
        "text/calendar" => "invite.ics",
        "message/rfc822" => "message.eml",
        _ => "",
    }
}

/// A message id or Content-ID without surrounding whitespace and angle brackets.
pub(crate) fn bare_id(id: &str) -> &str {
    id.trim().trim_start_matches('<').trim_end_matches('>')
}

/// First header with this name (case-insensitive) on a payload or part.
fn find_header<'a>(part: &'a Value, name: &str) -> Option<&'a str> {
    part["headers"].as_array()?.iter().find(|h| {
        h["name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
    })?["value"]
        .as_str()
}

/// Message ids from `Message-ID`, `In-Reply-To` or `References`, without angle brackets.
/// Falls back to whitespace-separated tokens when there are no brackets.
pub(crate) fn message_ids(value: &str) -> Vec<String> {
    let bracketed: Vec<String> = value
        .split('<')
        .skip(1)
        .filter_map(|s| s.split_once('>').map(|(id, _)| id.trim().to_string()))
        .filter(|id| !id.is_empty())
        .collect();
    if !bracketed.is_empty() || value.contains('<') {
        return bracketed;
    }
    value.split_whitespace().map(str::to_string).collect()
}

/// Parses an RFC 2822 `Date` header into Unix milliseconds. Tolerates trailing comments
/// such as `(UTC)` and extra whitespace.
pub(crate) fn parse_date(value: &str) -> Option<i64> {
    let without_comment = value.split_once('(').map_or(value, |(date, _)| date);
    chrono::DateTime::parse_from_rfc2822(&collapse_whitespace(without_comment))
        .ok()
        .map(|d| d.timestamp_millis())
}
