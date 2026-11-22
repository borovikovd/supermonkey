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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error("the draft id is not a header-safe token")]
    DraftId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutAttachment {
    pub filename: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlinePart {
    pub content_id: String,
    pub mime_type: String,
    pub filename: Option<String>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct Outgoing<'a> {
    pub from: &'a Addr,
    pub to: &'a [Addr],
    pub cc: &'a [Addr],
    pub bcc: &'a [Addr],
    pub subject: &'a str,
    pub html: &'a str,
    pub in_reply_to: Option<&'a str>,
    pub references: &'a [String],
    pub draft_id: &'a DraftId,
    pub attachments: &'a [OutAttachment],
    pub inline: &'a [InlinePart],
    pub date: Millis,
}

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

#[derive(Debug, Clone, Copy)]
pub struct PlainMessage<'a> {
    pub from: &'a Addr,
    pub to: &'a Addr,
    pub subject: &'a str,
    pub text: &'a str,
    pub date: Millis,
}

#[must_use]
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

fn id_domain(from: &Addr) -> &str {
    from.email
        .rsplit_once('@')
        .map(|(_, domain)| domain)
        .filter(|domain| is_message_id(domain))
        .unwrap_or("supermonkey.local")
}

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
