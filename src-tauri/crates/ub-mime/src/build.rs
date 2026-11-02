//! Building outgoing RFC 5322 messages.

use std::iter;

use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address;
use mail_builder::headers::content_type::ContentType;
use mail_builder::headers::date::Date;
use mail_builder::headers::message_id::MessageId;
use mail_builder::headers::raw::Raw;
use mail_builder::mime::MimePart;
use serde::{Deserialize, Serialize};

use crate::address::Addr;
use crate::gmail::bare_id;
use crate::html::html_to_plain;

/// A file attached to an outgoing message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutAttachment {
    pub filename: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

/// An inline part referenced from the HTML as `cid:<content_id>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InlinePart {
    /// Content-ID without angle brackets.
    pub content_id: String,
    pub mime_type: String,
    pub filename: Option<String>,
    pub data: Vec<u8>,
}

/// Everything needed to build an outgoing message or draft.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Outgoing {
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub bcc: Vec<Addr>,
    pub subject: String,
    /// The full HTML body: editor HTML followed by the quote block, with `cid:` references
    /// (see [`data_images_to_cid`](crate::data_images_to_cid)).
    pub html: String,
    /// Message ids, with or without angle brackets.
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// Written as the `X-Ubermensch-Draft` header.
    pub ub_draft_id: String,
    pub attachments: Vec<OutAttachment>,
    /// Inline parts placed in `multipart/related`; pass only those the HTML references.
    pub inline: Vec<InlinePart>,
    /// The `Date` header, Unix milliseconds.
    pub date_ms: i64,
    /// The domain of the generated `Message-ID` (`ub.<draft id>.<date_ms>@<domain>`).
    pub message_id_domain: String,
}

/// Builds the RFC 5322 bytes of `o`. The tree is
/// `multipart/mixed { multipart/related { multipart/alternative { text/plain, text/html },
/// inline parts… }, attachments… }`, with `multipart/related` dropped when there are no inline
/// parts and `multipart/mixed` dropped when there are no attachments. The `text/plain` part is
/// generated from the HTML. Non-ASCII names, subjects and filenames are encoded by mail-builder
/// (RFC 2047 / RFC 2231). Empty address lists are omitted; `Bcc` is written when present, as
/// Gmail needs it to deliver and then strips it.
pub fn build_mime(o: &Outgoing) -> Vec<u8> {
    let mut message = MessageBuilder::new().from(address(&o.from));
    if !o.to.is_empty() {
        message = message.to(address_list(&o.to));
    }
    if !o.cc.is_empty() {
        message = message.cc(address_list(&o.cc));
    }
    if !o.bcc.is_empty() {
        message = message.bcc(address_list(&o.bcc));
    }
    let draft_token: String = o
        .ub_draft_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    message = message
        .subject(o.subject.as_str())
        .date(Date::new(o.date_ms.div_euclid(1000)))
        .message_id(format!(
            "ub.{draft_token}.{}@{}",
            o.date_ms, o.message_id_domain
        ));
    if let Some(id) = &o.in_reply_to {
        message = message.in_reply_to(bare_id(id));
    }
    if !o.references.is_empty() {
        message = message.references(MessageId::new_list(o.references.iter().map(|r| bare_id(r))));
    }
    message
        .header("X-Ubermensch-Draft", Raw::new(o.ub_draft_id.as_str()))
        .body(body(o))
        .write_to_vec()
        .expect("writing to a Vec cannot fail")
}

/// The body tree described on [`build_mime`].
fn body(o: &Outgoing) -> MimePart<'_> {
    let mut body = MimePart::new(
        "multipart/alternative",
        vec![
            MimePart::new("text/plain", html_to_plain(&o.html)),
            MimePart::new("text/html", o.html.as_str()),
        ],
    );
    if !o.inline.is_empty() {
        let related = iter::once(body).chain(o.inline.iter().map(inline_part));
        body = MimePart::new("multipart/related", related.collect::<Vec<_>>());
    }
    if !o.attachments.is_empty() {
        let mixed = iter::once(body).chain(o.attachments.iter().map(attachment_part));
        body = MimePart::new("multipart/mixed", mixed.collect::<Vec<_>>());
    }
    body
}

fn inline_part(part: &InlinePart) -> MimePart<'_> {
    let mut content_type = ContentType::new(part.mime_type.as_str());
    let mut disposition = ContentType::new("inline");
    if let Some(name) = &part.filename {
        content_type = content_type.attribute("name", name.as_str());
        disposition = disposition.attribute("filename", name.as_str());
    }
    MimePart::new(content_type, part.data.as_slice())
        .header("Content-Disposition", disposition)
        .cid(bare_id(&part.content_id))
}

fn attachment_part(a: &OutAttachment) -> MimePart<'_> {
    let content_type =
        ContentType::new(a.mime_type.as_str()).attribute("name", a.filename.as_str());
    MimePart::new(content_type, a.data.as_slice()).attachment(a.filename.as_str())
}

fn address(a: &Addr) -> Address<'_> {
    Address::new_address(
        a.name.as_deref().filter(|n| !n.is_empty()),
        a.email.as_str(),
    )
}

fn address_list(list: &[Addr]) -> Address<'_> {
    Address::new_list(list.iter().map(address).collect())
}
