//! Outgoing RFC 5322 messages, built with mail-builder (SPEC Compose, Drafts).

use std::iter;

use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address;
use mail_builder::headers::content_type::ContentType;
use mail_builder::headers::date::Date;
use mail_builder::headers::message_id::MessageId;
use mail_builder::headers::raw::Raw;
use mail_builder::mime::MimePart;
use sm_domain::{Addr, DraftId, Millis};

use crate::headers::{DRAFT_HEADER, bare_id, is_draft_token, is_message_id};
use crate::text::html_to_plain;

/// A message that cannot be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    /// The local draft id is not a safe header token (see [`Outgoing::draft_id`]).
    #[error("the draft id is not a header-safe token")]
    DraftId,
}

/// A file attached to an outgoing message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutAttachment {
    /// The file's name, as the recipient sees it.
    pub filename: String,
    /// Its MIME type.
    pub mime_type: String,
    /// Its bytes.
    pub data: Vec<u8>,
}

/// An inline part, referenced from the HTML as `cid:<content_id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlinePart {
    /// The Content-ID, with or without angle brackets.
    pub content_id: String,
    /// Its MIME type.
    pub mime_type: String,
    /// Its file name, if it has one.
    pub filename: Option<String>,
    /// Its bytes.
    pub data: Vec<u8>,
}

/// A draft or message to write to Gmail.
#[derive(Debug, Clone, Copy)]
pub struct Outgoing<'a> {
    /// The sender. Its domain also names the `Message-ID`.
    pub from: &'a Addr,
    /// `To` recipients.
    pub to: &'a [Addr],
    /// `Cc` recipients.
    pub cc: &'a [Addr],
    /// `Bcc` recipients, written so Gmail can deliver to them; Gmail strips them.
    pub bcc: &'a [Addr],
    /// The subject.
    pub subject: &'a str,
    /// The whole HTML body: the editor's HTML, then the quote block, with
    /// `cid:` references (see [`data_images_to_cid`](crate::data_images_to_cid)).
    pub html: &'a str,
    /// The message replied to, with or without angle brackets.
    pub in_reply_to: Option<&'a str>,
    /// The thread's message ids, with or without angle brackets.
    pub references: &'a [String],
    /// The local draft id, written as `X-Supermonkey-Draft` (SPEC Drafts). It
    /// must be 1 to 128 ASCII letters, digits, `-`, `_` or `.`, as UUIDs are.
    pub draft_id: &'a DraftId,
    /// Attachments.
    pub attachments: &'a [OutAttachment],
    /// Inline parts. Pass only those the HTML references.
    pub inline: &'a [InlinePart],
    /// The `Date` header.
    pub date: Millis,
}

/// Builds the RFC 5322 bytes of `message`, for `drafts.create`,
/// `drafts.update`, and `messages.send` (base64url them into `raw`).
///
/// The tree is `multipart/mixed { multipart/related { multipart/alternative {
/// text/plain, text/html }, inline parts… }, attachments… }`, without
/// `multipart/mixed` when there are no attachments; `multipart/related` stays
/// even with no inline parts (SPEC Compose). The `text/plain` part is made from
/// the HTML. mail-builder encodes non-ASCII names, subjects, and filenames.
/// Empty address lists are left out. The `Message-ID` is
/// `ub.<draft id>.<date ms>@<sender's domain>`. Reply ids that could not be
/// written back safely are dropped.
pub fn build_mime(message: &Outgoing<'_>) -> Result<Vec<u8>, BuildError> {
    let draft_id = message.draft_id.as_str();
    if !is_draft_token(draft_id) {
        return Err(BuildError::DraftId);
    }
    let mut builder = MessageBuilder::new().from(address(message.from));
    if !message.to.is_empty() {
        builder = builder.to(address_list(message.to));
    }
    if !message.cc.is_empty() {
        builder = builder.cc(address_list(message.cc));
    }
    if !message.bcc.is_empty() {
        builder = builder.bcc(address_list(message.bcc));
    }
    builder = builder
        .subject(message.subject)
        .date(Date::new(message.date.0.div_euclid(1000)))
        .message_id(format!(
            "ub.{draft_id}.{}@{}",
            message.date,
            id_domain(message.from)
        ));
    if let Some(id) = message
        .in_reply_to
        .map(bare_id)
        .filter(|id| is_message_id(id))
    {
        builder = builder.in_reply_to(id);
    }
    let references: Vec<&str> = message
        .references
        .iter()
        .map(|id| bare_id(id))
        .filter(|id| is_message_id(id))
        .collect();
    if !references.is_empty() {
        builder = builder.references(MessageId::new_list(references.into_iter()));
    }
    let mut out = Vec::new();
    builder
        .header(DRAFT_HEADER, Raw::new(draft_id))
        .body(body_tree(message))
        .serialize(&mut out);
    Ok(out)
}

/// A plain-text message, such as a list's unsubscribe request (SPEC
/// Unsubscribe and block). It carries no draft header: it is not one of the app's drafts.
#[derive(Debug, Clone, Copy)]
pub struct PlainMessage<'a> {
    /// The sender. Its domain also names the `Message-ID`.
    pub from: &'a Addr,
    /// The one recipient.
    pub to: &'a Addr,
    /// The subject.
    pub subject: &'a str,
    /// The text body.
    pub text: &'a str,
    /// The `Date` header.
    pub date: Millis,
}

/// Builds the RFC 5322 bytes of a [`PlainMessage`], for `messages.send`. Its
/// `Message-ID` is `ub.unsubscribe.<date ms>@<sender's domain>`.
pub fn build_plain(message: &PlainMessage<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    MessageBuilder::new()
        .from(address(message.from))
        .to(address(message.to))
        .subject(message.subject)
        .date(Date::new(message.date.0.div_euclid(1000)))
        .message_id(format!(
            "ub.unsubscribe.{}@{}",
            message.date,
            id_domain(message.from)
        ))
        .text_body(message.text)
        .serialize(&mut out);
    out
}

/// The domain for generated `Message-ID`s: the sender's, when it has a safe one.
fn id_domain(from: &Addr) -> &str {
    from.email
        .rsplit_once('@')
        .map(|(_, domain)| domain)
        .filter(|domain| is_message_id(domain))
        .unwrap_or("supermonkey.local")
}

/// The body tree described on [`build_mime`].
fn body_tree<'a>(message: &Outgoing<'a>) -> MimePart<'a> {
    let alternative = MimePart::new(
        "multipart/alternative",
        vec![
            MimePart::new("text/plain", html_to_plain(message.html)),
            MimePart::new("text/html", message.html),
        ],
    );
    let related: Vec<MimePart<'a>> = iter::once(alternative)
        .chain(message.inline.iter().map(inline_part))
        .collect();
    let related = MimePart::new("multipart/related", related);
    if message.attachments.is_empty() {
        return related;
    }
    let mixed: Vec<MimePart<'a>> = iter::once(related)
        .chain(message.attachments.iter().map(attachment_part))
        .collect();
    MimePart::new("multipart/mixed", mixed)
}

/// `mime_type` when it is a plain `type/subtype` token, else
/// `application/octet-stream`: mail-builder writes it into the header as it is.
fn safe_mime_type(mime_type: &str) -> &str {
    let is_token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    match mime_type.split_once('/') {
        Some((kind, subtype)) if is_token(kind) && is_token(subtype) => mime_type,
        _ => "application/octet-stream",
    }
}

fn inline_part(part: &InlinePart) -> MimePart<'_> {
    let mut content_type = ContentType::new(safe_mime_type(&part.mime_type));
    let mut disposition = ContentType::new("inline");
    if let Some(name) = &part.filename {
        content_type = content_type.attribute("name", name.as_str());
        disposition = disposition.attribute("filename", name.as_str());
    }
    MimePart::new(content_type, part.data.as_slice())
        .header("Content-Disposition", disposition)
        .cid(bare_id(&part.content_id))
}

fn attachment_part(attachment: &OutAttachment) -> MimePart<'_> {
    let content_type = ContentType::new(safe_mime_type(&attachment.mime_type))
        .attribute("name", attachment.filename.as_str());
    MimePart::new(content_type, attachment.data.as_slice()).attachment(attachment.filename.as_str())
}

fn address(addr: &Addr) -> Address<'_> {
    Address::new_address(
        addr.name.as_deref().filter(|name| !name.is_empty()),
        addr.email.as_str(),
    )
}

fn address_list(list: &[Addr]) -> Address<'_> {
    Address::new_list(list.iter().map(address).collect())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn header_lines(raw: &[u8]) -> Vec<String> {
        let text = String::from_utf8_lossy(raw);
        let head = text.split("\r\n\r\n").next().unwrap_or_default();
        head.split("\r\n").map(str::to_owned).collect()
    }

    #[test]
    fn user_text_cannot_add_headers() {
        let evil = "x\r\nBcc: evil@attacker.example\r\nX-Injected: 1";
        let from = Addr::named(evil, "me@x.com");
        let to = [Addr::new(format!("b@y.org>{evil}"))];
        let references = [format!("r1@x>{evil}"), "<r2@y>".to_owned()];
        let draft_id = DraftId::from("6f1c-aa");
        let attachments = [OutAttachment {
            filename: evil.to_owned(),
            mime_type: format!("text/plain{evil}"),
            data: vec![1],
        }];
        let raw = build_mime(&Outgoing {
            from: &from,
            to: &to,
            cc: &[],
            bcc: &[],
            subject: evil,
            html: "<p>hi</p>",
            in_reply_to: Some(evil),
            references: &references,
            draft_id: &draft_id,
            attachments: &attachments,
            inline: &[],
            date: Millis(1_700_000_000_000),
        })
        .unwrap();
        let text = String::from_utf8_lossy(&raw);
        for line in text.split("\r\n") {
            assert!(!line.starts_with("Bcc:"), "{line:?} in\n{text}");
            assert!(!line.starts_with("X-Injected"), "{line:?} in\n{text}");
        }
        let lines = header_lines(&raw);
        assert!(
            lines.contains(&"References: <r2@y>".to_owned()),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.starts_with("In-Reply-To")),
            "{lines:?}"
        );
        assert!(
            text.contains("Content-Type: application/octet-stream"),
            "{text}"
        );
    }

    #[test]
    fn the_draft_header_takes_only_a_token() {
        let from = Addr::new("me@x.com");
        let build = |id: &str| {
            build_mime(&Outgoing {
                from: &from,
                to: &[],
                cc: &[],
                bcc: &[],
                subject: "",
                html: "",
                in_reply_to: None,
                references: &[],
                draft_id: &DraftId::from(id),
                attachments: &[],
                inline: &[],
                date: Millis(0),
            })
        };
        assert_eq!(build("a\r\nBcc: e@x.com"), Err(BuildError::DraftId));
        assert_eq!(build(""), Err(BuildError::DraftId));
        let raw = build("1b4e28ba-2fa1-11d2-883f-0016d3cca427").unwrap();
        let lines = header_lines(&raw);
        assert!(
            lines.contains(&"X-Supermonkey-Draft: 1b4e28ba-2fa1-11d2-883f-0016d3cca427".to_owned()),
            "{lines:?}"
        );

        let draft_id = |value: &str| {
            let payload = json!({ "headers": [{ "name": "x-supermonkey-draft", "value": value }] });
            crate::parse_message(&payload).unwrap().headers.draft_id
        };
        assert_eq!(draft_id(" d-1 "), Some(DraftId::from("d-1")));
        assert_eq!(draft_id("d 1"), None);
    }
}
