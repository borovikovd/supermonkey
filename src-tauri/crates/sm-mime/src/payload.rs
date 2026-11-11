//! The `payload` of a Gmail `users.messages` resource: its headers, bodies, and
//! attachment and inline parts.

use std::borrow::Cow;
use std::collections::HashSet;

use serde::Deserialize;
use sm_domain::{Addr, DraftId, Millis};

use crate::addr::parse_address_list;
use crate::body::RawBody;
use crate::charset::{decode_base64, decode_text_part};
use crate::cid::{CidImages, cid_references};
use crate::encoded_word::decode_encoded_words;
use crate::headers::{DRAFT_HEADER, bare_id, message_ids, parse_date, parse_draft_id};
use crate::params::{header_kind, header_param};

/// A payload that is not the shape Gmail documents.
#[derive(Debug, thiserror::Error)]
#[error("malformed Gmail message payload: {0}")]
pub struct PayloadError(#[from] serde_json::Error);

/// A received message, parsed. Nothing here is sanitized yet: render the body
/// with [`RawBody::render`] or [`RawBody::to_draft`].
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
    /// The local draft id from `X-Supermonkey-Draft`, on drafts this app wrote.
    pub draft_id: Option<DraftId>,
}

/// An attachment or inline part of a received message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Part {
    /// Gmail's `partId`, such as `1.2`.
    pub id: String,
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
    /// An inline image: an `image/*` part whose Content-ID the message's
    /// HTML body references (`cid:`), so the body shows it. Any other part
    /// is an attachment, whatever its disposition: Apple Mail sends every
    /// file `inline`, and a photo with no Content-ID, or one the HTML never
    /// references, would otherwise show nowhere.
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
    /// Inline images ([`Part::is_inline`]) that Gmail left out of the
    /// payload. Fetch each, add it to [`ParsedMessage::cid_images`], then render.
    pub fn inline_images_to_fetch(&self) -> impl Iterator<Item = InlineFetch<'_>> {
        self.parts.iter().filter_map(|part| {
            if part.data.is_some() || !part.is_inline {
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
    #[must_use]
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
/// moved behind an attachment id is skipped. Only the images the HTML body
/// shows are inline parts ([`Part::is_inline`]). A message attached to this
/// one (`message/rfc822`, as a forward as attachment sends it) adds the
/// attachments inside it, not its body or the images its own HTML body
/// shows; the attached message itself is listed only when Gmail gives it an
/// attachment id.
pub fn parse_message(payload: &serde_json::Value) -> Result<ParsedMessage, PayloadError> {
    let root = WirePart::deserialize(payload)?;
    let mut walk = Walk::new();
    walk.visit(&root, THIS_MESSAGE);
    let (body, parts) = walk.finish();
    Ok(ParsedMessage {
        headers: MessageHeaders::from_part(&root),
        body,
        parts,
    })
}

impl MessageHeaders {
    fn from_part(root: &WirePart<'_>) -> Self {
        let header = |name: &str| root.header(name);
        let addresses = |name: &str| header(name).map(parse_address_list).unwrap_or_default();
        let ids = |name: &str| header(name).map(message_ids).unwrap_or_default();
        let first_id = |name: &str| ids(name).first().map(|&id| id.to_owned());
        Self {
            from: addresses("From").into_iter().next(),
            to: addresses("To"),
            cc: addresses("Cc"),
            bcc: addresses("Bcc"),
            reply_to: addresses("Reply-To"),
            subject: header("Subject").map(subject).unwrap_or_default(),
            date: header("Date").and_then(parse_date),
            message_id: first_id("Message-ID"),
            in_reply_to: first_id("In-Reply-To"),
            references: ids("References").into_iter().map(str::to_owned).collect(),
            is_list: header("List-Id").is_some() || header("List-Unsubscribe").is_some(),
            draft_id: header(DRAFT_HEADER).and_then(parse_draft_id),
        }
    }
}

/// A `Subject` value, decoded and trimmed. Copies decoded text only when it
/// had whitespace to trim.
fn subject(value: &str) -> String {
    match decode_encoded_words(value) {
        Cow::Borrowed(as_written) => as_written.trim().to_owned(),
        Cow::Owned(decoded) => {
            let trimmed = decoded.trim();
            if trimmed.len() == decoded.len() {
                decoded
            } else {
                trimmed.to_owned()
            }
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
    parts: Vec<Self>,
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

/// The index of the message being parsed in [`Walk::messages`]; the messages
/// attached to it follow in document order.
const THIS_MESSAGE: usize = 0;

/// What a walk of the part tree collects, in document order.
struct Walk {
    html: Option<String>,
    text: Option<String>,
    /// Each part that is not a body, with the index of the message it is
    /// in, before [`Walk::finish`] decides which are inline.
    found: Vec<(usize, Part)>,
    /// The Content-IDs each message's HTML body references, by index: this
    /// message, then each attached one.
    messages: Vec<HashSet<String>>,
}

impl Walk {
    fn new() -> Self {
        Self {
            html: None,
            text: None,
            found: Vec::new(),
            messages: vec![HashSet::new()],
        }
    }

    /// Visits `part`, which is in message `message` (an index into
    /// [`Walk::messages`]).
    fn visit(&mut self, part: &WirePart<'_>, message: usize) {
        let mime_type = part.mime_type.trim().to_ascii_lowercase();
        if mime_type.starts_with("multipart/") {
            for child in &part.parts {
                self.visit(child, message);
            }
            return;
        }
        // Gmail serves an attached message's parts under it.
        let is_message = mime_type == "message/rfc822";
        self.leaf(part, mime_type, message);
        if is_message {
            let attached = self.messages.len();
            self.messages.push(HashSet::new());
            for child in &part.parts {
                self.visit(child, attached);
            }
        }
    }

    /// Ends the walk: this message's bodies, and the parts found, each image
    /// marked inline when its message's HTML body references it. An attached
    /// message's inline images belong to its body, which is not shown, so
    /// they are left out.
    fn finish(self) -> (RawBody, Vec<Part>) {
        let Self {
            html,
            text,
            found,
            messages,
        } = self;
        let parts = found
            .into_iter()
            .filter_map(|(message, mut part)| {
                part.is_inline = part.mime_type.starts_with("image/")
                    && part.content_id.as_ref().is_some_and(|id| {
                        messages
                            .get(message)
                            .is_some_and(|referenced| referenced.contains(id))
                    });
                (message == THIS_MESSAGE || !part.is_inline).then_some(part)
            })
            .collect();
        (RawBody { html, text }, parts)
    }

    /// A part that is not a container: a body, an attachment, or an inline part.
    fn leaf(&mut self, part: &WirePart<'_>, mime_type: String, message: usize) {
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
            && !disposition_kind.eq_ignore_ascii_case("attachment");
        if is_body {
            let is_html = mime_type == "text/html";
            // An attached message's HTML only says which of its images it shows.
            if message != THIS_MESSAGE && !is_html {
                return;
            }
            if let Some(bytes) = data {
                let charset = header_param(content_type, "charset");
                let decoded = decode_text_part(bytes, charset.as_deref());
                if is_html && let Some(referenced) = self.messages.get_mut(message) {
                    referenced.extend(cid_references(&decoded));
                }
                if message != THIS_MESSAGE {
                    return;
                }
                let body = if is_html {
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
        let filename = if filename.is_empty() {
            default_filename(&mime_type).to_owned()
        } else {
            filename
        };
        self.found.push((
            message,
            Part {
                id: part.part_id.to_owned(),
                filename,
                mime_type,
                size: part.body.size,
                attachment_id,
                content_id,
                // Decided once every body is in (`Walk::finish`).
                is_inline: false,
                data,
            },
        ));
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
