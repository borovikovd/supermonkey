use chrono::{DateTime, Datelike as _, FixedOffset, Timelike as _, Utc, Weekday};
use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address;
use mail_builder::headers::raw::Raw;
use mail_builder::mime::MimePart;
use thiserror::Error;

use crate::domain::{Addr, DraftId, Millis, is_forward_subject};

use super::html::html_to_plain;
use super::parse::{DRAFT_HEADER, is_draft_token, is_message_id};

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("invalid draft id for MIME header")]
    InvalidDraftId,
    #[error("invalid Message-ID header value")]
    InvalidMessageId,
    #[error("couldn't serialize MIME message: {0}")]
    Write(#[from] std::io::Error),
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
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct QuoteSource<'a> {
    pub from: &'a Addr,
    pub to: &'a [Addr],
    pub cc: &'a [Addr],
    pub subject: &'a str,
    pub date: Millis,
    pub html: &'a str,
}

#[derive(Debug)]
pub struct Outgoing<'a> {
    pub from: &'a Addr,
    pub to: &'a [Addr],
    pub cc: &'a [Addr],
    pub bcc: &'a [Addr],
    pub subject: &'a str,
    pub body_html: &'a str,
    pub quote_html: Option<&'a str>,
    pub in_reply_to: Option<&'a str>,
    pub references: &'a [String],
    pub draft_id: Option<&'a DraftId>,
    pub attachments: &'a [OutAttachment],
    pub inline: &'a [InlinePart],
    pub date: Millis,
}

#[derive(Debug)]
pub struct PlainMessage<'a> {
    pub from: &'a Addr,
    pub to: &'a Addr,
    pub subject: &'a str,
    pub body: &'a str,
    pub date: Millis,
}

#[must_use]
pub fn reply_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    if trimmed
        .get(..3)
        .is_some_and(|p| p.eq_ignore_ascii_case("re:"))
    {
        trimmed.to_owned()
    } else if trimmed.is_empty() {
        "Re:".to_owned()
    } else {
        format!("Re: {trimmed}")
    }
}

#[must_use]
pub fn forward_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    if is_forward_subject(trimmed) {
        trimmed.to_owned()
    } else if trimmed.is_empty() {
        "Fwd:".to_owned()
    } else {
        format!("Fwd: {trimmed}")
    }
}

#[must_use]
pub fn reply_quote_html(source: QuoteSource<'_>, offset: FixedOffset) -> String {
    let when = format_quote_date(source.date, offset);
    let who = format_quote_addr(source.from);
    let escaped_who = html_escape::encode_text(&who);
    format!(
        "<div class=\"gmail_quote\"><div dir=\"ltr\" class=\"gmail_attr\">On {when} {escaped_who} wrote:<br></div><blockquote class=\"gmail_quote\" style=\"margin:0px 0px 0px 0.8ex;border-left:1px solid rgb(204,204,204);padding-left:1ex\">{}</blockquote></div>",
        source.html
    )
}

#[must_use]
pub fn forward_quote_html(source: QuoteSource<'_>, offset: FixedOffset) -> String {
    let when = format_quote_date(source.date, offset);
    let from_raw = format_quote_addr(source.from);
    let from = html_escape::encode_text(&from_raw);
    let subject = html_escape::encode_text(source.subject);
    let to_raw = format_quote_list(source.to);
    let to = html_escape::encode_text(&to_raw);
    let cc_line = if source.cc.is_empty() {
        String::new()
    } else {
        let cc_raw = format_quote_list(source.cc);
        let cc = html_escape::encode_text(&cc_raw);
        format!("Cc: {cc}<br>")
    };
    format!(
        "<div class=\"gmail_quote\"><div dir=\"ltr\" class=\"gmail_attr\">---------- Forwarded message ---------<br>From: {from}<br>Date: {when}<br>Subject: {subject}<br>To: {to}<br>{cc_line}</div><br><br>{}</div>",
        source.html
    )
}

pub fn build_mime(msg: &Outgoing<'_>) -> Result<Vec<u8>, BuildError> {
    if let Some(draft) = msg.draft_id
        && !is_draft_token(draft.as_str())
    {
        return Err(BuildError::InvalidDraftId);
    }
    if let Some(irt) = msg.in_reply_to
        && !is_message_id(irt)
    {
        return Err(BuildError::InvalidMessageId);
    }
    for r in msg.references {
        if !is_message_id(r) {
            return Err(BuildError::InvalidMessageId);
        }
    }

    let mut builder = MessageBuilder::new()
        .from(to_mb_addr(msg.from))
        .subject(clean_header(msg.subject))
        .date(msg.date.0 / 1000);

    if !msg.to.is_empty() {
        builder = builder.to(to_mb_addrs(msg.to));
    }
    if !msg.cc.is_empty() {
        builder = builder.cc(to_mb_addrs(msg.cc));
    }
    if !msg.bcc.is_empty() {
        builder = builder.bcc(to_mb_addrs(msg.bcc));
    }

    let msg_id = make_message_id(msg.draft_id, msg.date, &msg.from.email);
    builder = builder.message_id(msg_id);

    if let Some(irt) = msg.in_reply_to {
        builder = builder.in_reply_to(strip_angles(irt));
    }
    if !msg.references.is_empty() {
        let refs: Vec<String> = msg.references.iter().map(|r| strip_angles(r)).collect();
        builder = builder.references(refs);
    }
    if let Some(draft) = msg.draft_id {
        builder = builder.header(DRAFT_HEADER, Raw::new(draft.as_str()));
    }

    let full_html = match msg.quote_html {
        Some(q) if !q.is_empty() => format!("{}<br>{q}", msg.body_html),
        _ => msg.body_html.to_owned(),
    };
    let plain = html_to_plain(&full_html);
    let body = build_body_tree(&plain, &full_html, msg.inline, msg.attachments);
    builder = builder.body(body);
    Ok(builder.write_to_vec()?)
}

pub fn build_plain(msg: &PlainMessage<'_>) -> Result<Vec<u8>, BuildError> {
    let domain = msg
        .from
        .email
        .split_once('@')
        .map_or("supermonkey.local", |(_, d)| d);
    let msg_id = format!("ub.unsubscribe.{}@{}", msg.date.0, clean_domain(domain));
    let bytes = MessageBuilder::new()
        .from(to_mb_addr(msg.from))
        .to(to_mb_addr(msg.to))
        .subject(clean_header(msg.subject))
        .date(msg.date.0 / 1000)
        .message_id(msg_id)
        .text_body(msg.body)
        .write_to_vec()?;
    Ok(bytes)
}

fn build_body_tree<'a>(
    plain: &'a str,
    html: &'a str,
    inline: &'a [InlinePart],
    attachments: &'a [OutAttachment],
) -> MimePart<'a> {
    let alt = MimePart::new(
        "multipart/alternative",
        vec![
            MimePart::new("text/plain", plain),
            MimePart::new("text/html", html),
        ],
    );
    let mut rel = Vec::with_capacity(inline.len() + 1);
    rel.push(alt);
    for part in inline {
        let mime = safe_mime_type(&part.mime_type);
        let cid = clean_header(&part.content_id);
        rel.push(MimePart::new(mime, part.data.as_slice()).inline().cid(cid));
    }
    let core = MimePart::new("multipart/related", rel);
    if attachments.is_empty() {
        core
    } else {
        let mut mixed = Vec::with_capacity(attachments.len() + 1);
        mixed.push(core);
        for att in attachments {
            let mime = safe_mime_type(&att.mime_type);
            let name = clean_header(&att.filename);
            mixed.push(MimePart::new(mime, att.data.as_slice()).attachment(name));
        }
        MimePart::new("multipart/mixed", mixed)
    }
}

fn to_mb_addr(addr: &Addr) -> Address<'static> {
    let email = clean_header(&addr.email);
    match addr.name.as_deref().map(clean_header) {
        Some(name) if !name.is_empty() => Address::new_address(Some(name), email),
        _ => Address::new_address(None::<String>, email),
    }
}

fn to_mb_addrs(addrs: &[Addr]) -> Address<'static> {
    Address::new_list(addrs.iter().map(to_mb_addr).collect())
}

fn make_message_id(draft_id: Option<&DraftId>, date: Millis, from_email: &str) -> String {
    let domain = from_email
        .split_once('@')
        .map_or("supermonkey.local", |(_, d)| d);
    let clean_dom = clean_domain(domain);
    match draft_id {
        Some(id) => format!("ub.{}.{}@{clean_dom}", id.as_str(), date.0),
        None => format!("ub.{}@{clean_dom}", date.0),
    }
}

fn clean_header(s: &str) -> String {
    s.chars()
        .filter(|&c| c != '\r' && c != '\n' && c != '\0')
        .collect()
}

fn clean_domain(domain: &str) -> String {
    let cleaned: String = domain
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        .collect();
    if cleaned.is_empty() {
        "supermonkey.local".to_owned()
    } else {
        cleaned
    }
}

fn strip_angles(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_owned()
}

fn safe_mime_type(mime: &str) -> String {
    let trimmed = mime.trim();
    if let Some((top, sub)) = trimmed.split_once('/')
        && !top.is_empty()
        && !sub.is_empty()
        && !sub.contains('/')
        && is_mime_token(top)
        && is_mime_token(sub)
    {
        return format!("{}/{}", top.to_ascii_lowercase(), sub.to_ascii_lowercase());
    }
    "application/octet-stream".to_owned()
}

fn is_mime_token(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'+' | b'.' | b'_'))
}

fn format_quote_addr(addr: &Addr) -> String {
    match addr.name.as_deref().filter(|n| !n.is_empty()) {
        Some(name) => format!("{name} <{}>", addr.email),
        None => addr.email.clone(),
    }
}

fn format_quote_list(addrs: &[Addr]) -> String {
    addrs
        .iter()
        .map(format_quote_addr)
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_quote_date(ms: Millis, offset: FixedOffset) -> String {
    let dt = DateTime::<Utc>::from_timestamp_millis(ms.0)
        .unwrap_or(DateTime::UNIX_EPOCH)
        .with_timezone(&offset);
    let (pm, hour12) = dt.hour12();
    let ampm = if pm { "PM" } else { "AM" };
    let wd = match dt.weekday() {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    };
    let mo = match dt.month() {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    };
    format!(
        "{wd}, {mo} {}, {} at {}:{:02} {ampm}",
        dt.day(),
        dt.year(),
        hour12,
        dt.minute()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_text_cannot_inject_headers_into_outgoing_mime() {
        let from = Addr::named("Ana\r\nBcc: evil@attacker.test", "ana@acme.test");
        let to = [Addr::named("Bob\nX-Injected: yes", "bob@x.test")];
        let att = [OutAttachment {
            filename: "doc.pdf\r\nX-Evil: 1".to_owned(),
            mime_type: "application/pdf\r\nX-Bad: 2".to_owned(),
            data: b"pdf".to_vec(),
        }];
        let bytes = build_mime(&Outgoing {
            from: &from,
            to: &to,
            cc: &[],
            bcc: &[],
            subject: "Hi\r\nX-Injected: bad",
            body_html: "<p>Hello</p>",
            quote_html: None,
            in_reply_to: None,
            references: &[],
            draft_id: None,
            attachments: &att,
            inline: &[],
            date: Millis(1_700_000_000_000),
        })
        .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("\nX-Injected:"), "no injected header");
        assert!(!text.contains("\nBcc: evil"), "no injected Bcc");
        assert!(!text.contains("\nX-Evil:"), "no injected attachment header");
        assert!(!text.contains("\nX-Bad:"), "no injected mime header");
    }

    #[test]
    fn wraps_alternative_in_multipart_related() {
        let from = Addr::new("ana@acme.test");
        let to = [Addr::new("bob@x.test")];
        let bytes = build_mime(&Outgoing {
            from: &from,
            to: &to,
            cc: &[],
            bcc: &[],
            subject: "Simple",
            body_html: "<p>Hi</p>",
            quote_html: None,
            in_reply_to: None,
            references: &[],
            draft_id: None,
            attachments: &[],
            inline: &[],
            date: Millis(1_700_000_000_000),
        })
        .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("multipart/related"),
            "has multipart/related wrapper"
        );
        assert!(
            text.contains("multipart/alternative"),
            "has multipart/alternative"
        );
    }
}
