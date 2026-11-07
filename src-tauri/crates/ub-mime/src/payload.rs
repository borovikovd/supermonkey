//! The `payload` of a Gmail `users.messages` resource: its headers, bodies, and
//! attachment and inline parts.

use serde::Deserialize;
use ub_domain::{Addr, DraftId, Millis};

use crate::addr::parse_address_list;
use crate::body::RawBody;
use crate::charset::{decode_base64, decode_text_part};
use crate::cid::CidImages;
use crate::encoded_word::decode_encoded_words;
use crate::headers::{DRAFT_HEADER, bare_id, message_ids, parse_date, parse_draft_id};
use crate::params::{header_kind, header_param};

/// A payload that is not the shape Gmail documents.
#[derive(Debug, thiserror::Error)]
#[error("malformed Gmail message payload: {0}")]
pub struct PayloadError(#[from] serde_json::Error);

/// A received message, parsed. Nothing here is sanitized yet: render the body
/// with [`RawBody::render`] or [`RawBody::into_draft`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedMessage {
    /// The headers the app stores.
    pub headers: MessageHeaders,
    /// The decoded bodies; empty for `format=metadata`.
    pub body: RawBody,
    /// Attachments and inline parts in document order. Body parts are not here.
    pub parts: Vec<Part>,
}

/// The headers of a received message the app uses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageHeaders {
    /// The first `From` mailbox.
    pub from: Option<Addr>,
    /// `To` mailboxes.
    pub to: Vec<Addr>,
    /// `Cc` mailboxes.
    pub cc: Vec<Addr>,
    /// `Bcc` mailboxes (present on mail this account sent).
    pub bcc: Vec<Addr>,
    /// `Reply-To` mailboxes.
    pub reply_to: Vec<Addr>,
    /// The decoded, trimmed subject; empty when there is none.
    pub subject: String,
    /// The `Date` header, when it parses.
    pub date: Option<Millis>,
    /// `Message-ID`, without angle brackets.
    pub message_id: Option<String>,
    /// The first `In-Reply-To` id, without angle brackets.
    pub in_reply_to: Option<String>,
    /// `References` ids in order, without angle brackets.
    pub references: Vec<String>,
    /// `List-Id` or `List-Unsubscribe` is present (SPEC Split Inbox).
    pub is_list: bool,
    /// The local draft id from `X-Ubermensch-Draft`, on drafts this app wrote.
    pub draft_id: Option<DraftId>,
}

/// An attachment or inline part of a received message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Part {
    /// Gmail's `partId`, such as `1.2`.
    pub part_id: String,
    /// The decoded filename. A nameless calendar part is `invite.ics`, an
    /// attached message `message.eml`, anything else empty.
    pub filename: String,
    /// The lowercased MIME type, such as `image/png`.
    pub mime_type: String,
    /// Decoded size in bytes, as Gmail reports it.
    pub size: u64,
    /// Gmail's attachment id, when the bytes need `messages.attachments.get`.
    pub attachment_id: Option<String>,
    /// The Content-ID, without angle brackets.
    pub content_id: Option<String>,
    /// `Content-Disposition: inline`, or no disposition but a Content-ID.
    pub is_inline: bool,
    /// The bytes, when Gmail sent them in the payload.
    pub data: Option<Vec<u8>>,
}

/// An inline image whose bytes must be fetched before the body can show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineFetch<'a> {
    /// The image's Content-ID, for [`CidImages::insert`].
    pub content_id: &'a str,
    /// Gmail's attachment id, for `messages.attachments.get`.
    pub attachment_id: &'a str,
    /// The image's MIME type, for [`CidImages::insert`].
    pub mime_type: &'a str,
}

impl ParsedMessage {
    /// Inline images referenced by Content-ID that Gmail left out of the
    /// payload. Fetch each, add it to [`ParsedMessage::cid_images`], then render.
    pub fn inline_images_to_fetch(&self) -> impl Iterator<Item = InlineFetch<'_>> {
        self.parts.iter().filter_map(|part| {
            if part.data.is_some() || !part.mime_type.starts_with("image/") {
                return None;
            }
            Some(InlineFetch {
                content_id: part.content_id.as_deref()?,
                attachment_id: part.attachment_id.as_deref()?,
                mime_type: &part.mime_type,
            })
        })
    }

    /// The inline parts whose bytes came in the payload, by Content-ID.
    pub fn cid_images(&self) -> CidImages {
        let mut images = CidImages::new();
        for part in &self.parts {
            if let (Some(content_id), Some(data)) = (&part.content_id, &part.data) {
                images.insert(content_id, &part.mime_type, data);
            }
        }
        images
    }
}

/// Parses the `payload` of a Gmail `users.messages` resource, from
/// `format=full` or `format=metadata` (headers only, so no bodies or parts).
///
/// Bodies are the base64url `body.data` of `text/html` and `text/plain` parts
/// without a filename, decoded in their charset; several of one type (Apple
/// Mail splits a body around attachments) are joined in order. A body Gmail
/// moved behind an attachment id is skipped. A message attached to this one
/// (`message/rfc822`, as a forward as attachment sends it) adds the attachments
/// inside it, not its body or inline images; the attached message itself is
/// listed only when Gmail gives it an attachment id.
pub fn parse_message(payload: &serde_json::Value) -> Result<ParsedMessage, PayloadError> {
    let root = WirePart::deserialize(payload)?;
    let mut walk = Walk::default();
    walk.visit(&root, false);
    Ok(ParsedMessage {
        headers: MessageHeaders::from_part(&root),
        body: RawBody {
            html: walk.html,
            text: walk.text,
        },
        parts: walk.parts,
    })
}

impl MessageHeaders {
    fn from_part(root: &WirePart<'_>) -> Self {
        let header = |name: &str| root.header(name);
        let addresses = |name: &str| header(name).map(parse_address_list).unwrap_or_default();
        let first_id = |name: &str| header(name).and_then(|v| message_ids(v).into_iter().next());
        Self {
            from: addresses("From").into_iter().next(),
            to: addresses("To"),
            cc: addresses("Cc"),
            bcc: addresses("Bcc"),
            reply_to: addresses("Reply-To"),
            subject: header("Subject")
                .map(|s| decode_encoded_words(s).trim().to_owned())
                .unwrap_or_default(),
            date: header("Date").and_then(parse_date),
            message_id: first_id("Message-ID"),
            in_reply_to: first_id("In-Reply-To"),
            references: header("References").map(message_ids).unwrap_or_default(),
            is_list: header("List-Id").is_some() || header("List-Unsubscribe").is_some(),
            draft_id: header(DRAFT_HEADER).and_then(parse_draft_id),
        }
    }
}

/// Gmail's `MessagePart`, borrowed from the JSON.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePart<'a> {
    #[serde(default)]
    part_id: &'a str,
    #[serde(default)]
    mime_type: &'a str,
    #[serde(default)]
    filename: &'a str,
    #[serde(default, borrow)]
    headers: Vec<WireHeader<'a>>,
    #[serde(default, borrow)]
    body: WireBody<'a>,
    #[serde(default, borrow)]
    parts: Vec<WirePart<'a>>,
}

#[derive(Deserialize)]
struct WireHeader<'a> {
    name: &'a str,
    value: &'a str,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct WireBody<'a> {
    #[serde(default)]
    size: u64,
    #[serde(default)]
    data: Option<&'a str>,
    #[serde(default)]
    attachment_id: Option<&'a str>,
}

impl WirePart<'_> {
    /// The first header with this name, in any case.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value)
    }
}

/// What a walk of the part tree collects, in document order.
#[derive(Default)]
struct Walk {
    html: Option<String>,
    text: Option<String>,
    parts: Vec<Part>,
}

impl Walk {
    /// Visits `part`; `attached` when it is inside an attached message.
    fn visit(&mut self, part: &WirePart<'_>, attached: bool) {
        let mime_type = part.mime_type.trim().to_ascii_lowercase();
        if mime_type.starts_with("multipart/") {
            for child in &part.parts {
                self.visit(child, attached);
            }
            return;
        }
        // Gmail serves an attached message's parts under it.
        let is_message = mime_type == "message/rfc822";
        self.leaf(part, mime_type, attached);
        if is_message {
            for child in &part.parts {
                self.visit(child, true);
            }
        }
    }

    /// A part that is not a container: a body, an attachment, or an inline part.
    fn leaf(&mut self, part: &WirePart<'_>, mime_type: String, attached: bool) {
        let content_type = part.header("Content-Type").unwrap_or_default();
        let disposition = part.header("Content-Disposition").unwrap_or_default();
        let disposition_kind = header_kind(disposition);
        let filename = Some(part.filename)
            .filter(|f| !f.is_empty())
            .map(str::to_owned)
            .or_else(|| header_param(disposition, "filename").map(Into::into))
            .or_else(|| header_param(content_type, "name").map(Into::into))
            .unwrap_or_default();
        let data = part.body.data.and_then(decode_base64);

        let is_body = matches!(mime_type.as_str(), "text/html" | "text/plain")
            && filename.is_empty()
            && disposition_kind != "attachment";
        if is_body {
            if attached {
                return;
            }
            if let Some(bytes) = data {
                let charset = header_param(content_type, "charset");
                let decoded = decode_text_part(bytes, charset.as_deref());
                let body = if mime_type == "text/html" {
                    &mut self.html
                } else {
                    &mut self.text
                };
                match body {
                    Some(joined) => {
                        joined.push('\n');
                        joined.push_str(&decoded);
                    }
                    None => *body = Some(decoded),
                }
            }
            return;
        }

        let attachment_id = part.body.attachment_id.map(str::to_owned);
        if data.is_none() && attachment_id.is_none() {
            return;
        }
        let content_id = part
            .header("Content-ID")
            .map(bare_id)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        let is_inline = match disposition_kind.as_str() {
            "inline" => true,
            "" => content_id.is_some(),
            _ => false,
        };
        // An attached message's inline parts belong to its body, not shown.
        if attached && is_inline {
            return;
        }
        let filename = if filename.is_empty() {
            default_filename(&mime_type).to_owned()
        } else {
            filename
        };
        self.parts.push(Part {
            part_id: part.part_id.to_owned(),
            filename,
            mime_type,
            size: part.body.size,
            attachment_id,
            content_id,
            is_inline,
            data,
        });
    }
}

/// The name a nameless part is saved under; empty when its type has none.
fn default_filename(mime_type: &str) -> &'static str {
    match mime_type {
        "text/calendar" => "invite.ics",
        "message/rfc822" => "message.eml",
        _ => "",
    }
}
