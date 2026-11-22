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

#[derive(Debug, thiserror::Error)]
#[error("malformed Gmail message payload: {0}")]
pub struct PayloadError(#[from] serde_json::Error);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedMessage {
    pub headers: MessageHeaders,
    pub body: RawBody,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageHeaders {
    pub from: Option<Addr>,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub reply_to: Vec<Addr>,
    pub subject: String,
    pub date: Option<Millis>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub is_list: bool,
    pub draft_id: Option<DraftId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Part {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
    pub attachment_id: Option<String>,
    pub content_id: Option<String>,
    pub is_inline: bool,
    pub data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineFetch<'a> {
    pub content_id: &'a str,
    pub attachment_id: &'a str,
    pub mime_type: &'a str,
}

impl ParsedMessage {
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
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value)
    }
}

const THIS_MESSAGE: usize = 0;

struct Walk {
    html: Option<String>,
    text: Option<String>,
    found: Vec<(usize, Part)>,
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

    fn visit(&mut self, part: &WirePart<'_>, message: usize) {
        let mime_type = part.mime_type.trim().to_ascii_lowercase();
        if mime_type.starts_with("multipart/") {
            for child in &part.parts {
                self.visit(child, message);
            }
            return;
        }
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
                is_inline: false,
                data,
            },
        ));
    }
}

fn default_filename(mime_type: &str) -> &'static str {
    match mime_type {
        "text/calendar" => "invite.ics",
        "message/rfc822" => "message.eml",
        _ => "",
    }
}
