//! Just enough MIME for the fake: build RFC 822 from simple fields, and parse RFC 822 into
//! Gmail's `payload` JSON (part ids, headers, base64url bodies, attachment ids).

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE};
use serde_json::{Value, json};
use std::fmt::Write as _;

use crate::{NewAttachment, NewMessage};

/// A parsed message.
pub(crate) struct Parsed {
    pub payload: Value,
    pub headers: Vec<(String, String)>,
    /// Attachment id and bytes of every part with a filename.
    pub attachments: Vec<(String, Vec<u8>)>,
    /// The first text/plain body, else the first text/html body without tags.
    pub text: String,
}

/// Parses RFC 822 bytes. `new_id` names attachments.
pub(crate) fn parse(raw: &[u8], new_id: &mut dyn FnMut() -> String) -> Parsed {
    let mut ctx = Ctx {
        new_id,
        attachments: Vec::new(),
        plain: None,
        html: None,
    };
    let (payload, headers) = parse_part(raw, "", &mut ctx);
    let text = ctx
        .plain
        .or_else(|| ctx.html.map(|h| strip_tags(&h)))
        .unwrap_or_default();
    Parsed {
        payload,
        headers,
        attachments: ctx.attachments,
        text,
    }
}

/// Marks a part whose bytes the payload carries, as Gmail serves some small
/// parts. The fake's own header: left out of the payload's headers.
const SERVED_INLINE: &str = "X-Fake-Gmail-Served-Inline";

struct Ctx<'a> {
    new_id: &'a mut dyn FnMut() -> String,
    attachments: Vec<(String, Vec<u8>)>,
    plain: Option<String>,
    html: Option<String>,
}

fn parse_part(bytes: &[u8], part_id: &str, ctx: &mut Ctx<'_>) -> (Value, Vec<(String, String)>) {
    let (head, body) = split_head(bytes);
    let headers = parse_headers(&String::from_utf8_lossy(head));
    let (mime_type, params) = header(&headers, "Content-Type")
        .map_or_else(|| ("text/plain".into(), Vec::new()), parse_params);
    let filename = header(&headers, "Content-Disposition")
        .map(parse_params)
        .and_then(|(_, p)| param(&p, "filename"))
        .or_else(|| param(&params, "name"))
        .unwrap_or_default();
    let json_headers: Vec<Value> = headers
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case(SERVED_INLINE))
        .map(|(name, value)| json!({ "name": name, "value": value }))
        .collect();

    let mut node = json!({
        "partId": part_id,
        "mimeType": mime_type,
        "filename": filename,
        "headers": json_headers,
    });
    if let (true, Some(boundary)) = (
        mime_type.starts_with("multipart/"),
        param(&params, "boundary"),
    ) {
        let parts: Vec<Value> = split_multipart(body, &boundary)
            .into_iter()
            .enumerate()
            .map(|(i, part)| {
                let id = if part_id.is_empty() {
                    i.to_string()
                } else {
                    format!("{part_id}.{i}")
                };
                parse_part(part, &id, ctx).0
            })
            .collect();
        node["body"] = json!({ "size": 0 });
        node["parts"] = Value::Array(parts);
    } else {
        let encoding = header(&headers, "Content-Transfer-Encoding").unwrap_or("7bit");
        let data = decode_transfer(body, encoding);
        if filename.is_empty() {
            if mime_type == "text/plain" && ctx.plain.is_none() {
                ctx.plain = Some(String::from_utf8_lossy(&data).into_owned());
            } else if mime_type == "text/html" && ctx.html.is_none() {
                ctx.html = Some(String::from_utf8_lossy(&data).into_owned());
            }
            node["body"] = json!({ "size": data.len(), "data": URL_SAFE.encode(&data) });
        } else if header(&headers, SERVED_INLINE).is_some() {
            // A file whose bytes the payload carries, with no attachment id.
            node["body"] = json!({ "size": data.len(), "data": URL_SAFE.encode(&data) });
        } else {
            let id = (ctx.new_id)();
            node["body"] = json!({ "attachmentId": id, "size": data.len() });
            ctx.attachments.push((id, data));
        }
    }
    (node, headers)
}

fn split_head(bytes: &[u8]) -> (&[u8], &[u8]) {
    for (i, w) in bytes.windows(2).enumerate() {
        if w == b"\n\n" {
            return (&bytes[..i], &bytes[i + 2..]);
        }
        if w == b"\n\r" && bytes.get(i + 2) == Some(&b'\n') {
            return (&bytes[..i], &bytes[i + 3..]);
        }
    }
    (bytes, &[])
}

/// Unfolds and splits a header block. Values have RFC 2047 words decoded.
fn parse_headers(head: &str) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in head.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = headers.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    for (_, value) in &mut headers {
        *value = decode_words(value);
    }
    headers
}

/// The first header named `name`, case-insensitively.
pub(crate) fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// `type/sub; key=value; ...` into the lowercased type and its params.
fn parse_params(value: &str) -> (String, Vec<(String, String)>) {
    let mut pieces = value.split(';');
    let first = pieces
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let params = pieces
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| {
            (
                k.trim().to_ascii_lowercase(),
                v.trim().trim_matches('"').to_owned(),
            )
        })
        .collect();
    (first, params)
}

fn param(params: &[(String, String)], name: &str) -> Option<String> {
    params
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
}

fn split_multipart<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let delimiter = format!("--{boundary}");
    let closing = format!("--{boundary}--");
    let mut parts = Vec::new();
    let mut start: Option<usize> = None;
    let mut pos = 0;
    while pos < body.len() {
        let end = body[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(body.len(), |i| pos + i);
        let line = String::from_utf8_lossy(&body[pos..end]);
        let line = line.trim_end();
        if line == delimiter || line == closing {
            if let Some(s) = start {
                // The line break before a delimiter belongs to the delimiter.
                let mut part_end = pos.max(s);
                if part_end > s && body[part_end - 1] == b'\n' {
                    part_end -= 1;
                }
                if part_end > s && body[part_end - 1] == b'\r' {
                    part_end -= 1;
                }
                parts.push(&body[s..part_end]);
            }
            if line == closing {
                break;
            }
            start = Some((end + 1).min(body.len()));
        }
        pos = end + 1;
    }
    parts
}

fn decode_transfer(body: &[u8], encoding: &str) -> Vec<u8> {
    match encoding.to_ascii_lowercase().as_str() {
        "base64" => {
            let clean: Vec<u8> = body
                .iter()
                .copied()
                .filter(|b| !b.is_ascii_whitespace())
                .collect();
            STANDARD.decode(&clean).unwrap_or_default()
        }
        "quoted-printable" => decode_qp(body),
        _ => body.to_vec(),
    }
}

fn decode_qp(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        if body[i] == b'=' {
            let rest = &body[i + 1..];
            if rest.starts_with(b"\r\n") {
                i += 3;
                continue;
            }
            if rest.starts_with(b"\n") {
                i += 2;
                continue;
            }
            if let Some(byte) = rest
                .get(..2)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(body[i]);
        i += 1;
    }
    out
}

/// Decodes RFC 2047 encoded words in UTF-8 or US-ASCII; others are left as they are.
fn decode_words(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    let mut last_was_word = false;
    while let Some(start) = rest.find("=?") {
        let decoded = rest[start + 2..].find("?=").and_then(|len| {
            let inner = &rest[start + 2..start + 2 + len];
            let mut fields = inner.splitn(3, '?');
            let (charset, enc, text) = (fields.next()?, fields.next()?, fields.next()?);
            if !matches!(charset.to_ascii_lowercase().as_str(), "utf-8" | "us-ascii") {
                return None;
            }
            let bytes = match enc {
                "B" | "b" => STANDARD.decode(text).ok()?,
                "Q" | "q" => decode_qp(text.replace('_', " ").as_bytes()),
                _ => return None,
            };
            Some((
                String::from_utf8_lossy(&bytes).into_owned(),
                start + 2 + len + 2,
            ))
        });
        if let Some((text, end)) = decoded {
            let between = &rest[..start];
            if !(last_was_word && between.trim().is_empty()) {
                out.push_str(between);
            }
            out.push_str(&text);
            rest = &rest[end..];
            last_was_word = true;
        } else {
            out.push_str(&rest[..start + 2]);
            rest = &rest[start + 2..];
            last_was_word = false;
        }
    }
    out.push_str(rest);
    out
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// The snippet Gmail would show: collapsed whitespace, at most 200 characters,
/// about what Gmail returns, so a snippet wraps over lines as real ones do.
pub(crate) fn snippet(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
}

/// Builds RFC 822 bytes from simple fields. Every leaf is base64.
pub(crate) fn build(msg: &NewMessage, date_ms: u64, message_id: &str) -> Vec<u8> {
    let mut out = String::new();
    let mut push = |name: &str, value: &str| {
        if !value.is_empty() {
            let _ = write!(out, "{name}: {value}\r\n");
        }
    };
    push("From", &msg.from);
    push("To", &msg.to.join(", "));
    push("Cc", &msg.cc.join(", "));
    push("Subject", &encode_word(&msg.subject));
    push("Date", &rfc2822_date(date_ms));
    push("Message-ID", message_id);
    push(
        "In-Reply-To",
        msg.in_reply_to.as_deref().unwrap_or_default(),
    );
    push("References", msg.references.as_deref().unwrap_or_default());
    push("List-Id", msg.list_id.as_deref().unwrap_or_default());
    for (name, value) in &msg.extra_headers {
        push(name, value);
    }
    push("MIME-Version", "1.0");

    let mut counter = 0;
    let mut boundary = || {
        counter += 1;
        format!("=_fake_boundary_{counter}")
    };
    let charset = msg.charset.as_deref().unwrap_or("utf-8");
    let text_part =
        |mime: &str, text: &str| leaf(&format!("{mime}; charset={charset}"), text.as_bytes(), &[]);
    let mut body = match (&msg.text, &msg.html) {
        (Some(text), Some(html)) => multipart(
            "alternative",
            &boundary(),
            &[text_part("text/plain", text), text_part("text/html", html)],
        ),
        (None, Some(html)) => text_part("text/html", html),
        (text, None) => text_part("text/plain", text.as_deref().unwrap_or_default()),
    };
    let (inline, attached): (Vec<&NewAttachment>, Vec<&NewAttachment>) =
        msg.attachments.iter().partition(|a| a.content_id.is_some());
    if !inline.is_empty() {
        let mut parts = vec![body];
        parts.extend(inline.iter().map(|a| attachment(a)));
        body = multipart("related", &boundary(), &parts);
    }
    if !attached.is_empty() {
        let mut parts = vec![body];
        parts.extend(attached.iter().map(|a| attachment(a)));
        body = multipart("mixed", &boundary(), &parts);
    }
    out.push_str(&body);
    out.into_bytes()
}

fn attachment(a: &NewAttachment) -> String {
    let disposition = if a.content_id.is_some() || a.inline {
        "inline"
    } else {
        "attachment"
    };
    let mut headers = vec![(
        "Content-Disposition".to_owned(),
        format!("{disposition}; filename=\"{}\"", a.filename),
    )];
    if let Some(cid) = &a.content_id {
        headers.push(("Content-ID".to_owned(), format!("<{cid}>")));
    }
    if a.served_inline {
        headers.push((SERVED_INLINE.to_owned(), "yes".to_owned()));
    }
    leaf(
        &format!("{}; name=\"{}\"", a.mime_type, a.filename),
        &a.data,
        &headers,
    )
}

/// A part with headers and a base64 body.
fn leaf(content_type: &str, data: &[u8], extra: &[(String, String)]) -> String {
    let mut out = format!("Content-Type: {content_type}\r\nContent-Transfer-Encoding: base64\r\n");
    for (name, value) in extra {
        let _ = write!(out, "{name}: {value}\r\n");
    }
    out.push_str("\r\n");
    let encoded = STANDARD.encode(data);
    for chunk in encoded.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(chunk).expect("base64 is ascii"));
        out.push_str("\r\n");
    }
    out
}

fn multipart(subtype: &str, boundary: &str, parts: &[String]) -> String {
    let mut out = format!("Content-Type: multipart/{subtype}; boundary=\"{boundary}\"\r\n\r\n");
    for part in parts {
        let _ = write!(out, "--{boundary}\r\n{part}\r\n");
    }
    let _ = write!(out, "--{boundary}--\r\n");
    out
}

fn encode_word(text: &str) -> String {
    if text.is_ascii() {
        text.to_owned()
    } else {
        format!("=?utf-8?B?{}?=", STANDARD.encode(text))
    }
}

/// `Thu, 01 Jan 1970 00:00:00 +0000` for milliseconds since the epoch.
fn rfc2822_date(ms: u64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = ms / 1000;
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} +0000",
        DAYS[usize::try_from(days.rem_euclid(7)).unwrap_or(0)],
        day,
        MONTHS[usize::try_from(month - 1).unwrap_or(0)],
        year,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}
