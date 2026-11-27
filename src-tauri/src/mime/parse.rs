use std::borrow::Cow;
use std::collections::HashSet;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
use chrono::DateTime;
use encoding_rs::Encoding;
use percent_encoding::percent_decode_str;
use serde::Deserialize;
use thiserror::Error;

use crate::domain::{Addr, DraftId, GmailAttachmentId, Millis};

use super::html::{RawBody, collect_cid_refs};

pub const DRAFT_HEADER: &str = "X-Supermonkey-Draft";

#[derive(Debug, Error)]
pub enum PayloadError {
    #[error("invalid Gmail message payload: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WirePart {
    #[serde(default)]
    pub part_id: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub headers: Vec<WireHeader>,
    #[serde(default)]
    pub body: WireBody,
    #[serde(default)]
    pub parts: Vec<Self>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct WireHeader {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireBody {
    #[serde(default)]
    pub attachment_id: Option<GmailAttachmentId>,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub data: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MessageHeaders {
    pub from: Vec<Addr>,
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
    pub list_unsubscribe: Option<String>,
    pub list_unsubscribe_post: Option<String>,
    pub draft_id: Option<DraftId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub id: String,
    pub filename: String,
    pub mime_type: String,
    pub size: i64,
    pub attachment_id: Option<GmailAttachmentId>,
    pub content_id: Option<String>,
    pub is_inline: bool,
    pub inline_data: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineFetch<'a> {
    pub content_id: &'a str,
    pub mime_type: &'a str,
    pub attachment_id: Option<&'a GmailAttachmentId>,
    pub inline_data: Option<&'a [u8]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    pub headers: MessageHeaders,
    pub raw_body: RawBody,
    pub parts: Vec<Part>,
}

impl ParsedMessage {
    pub fn inline_fetches(&self) -> impl Iterator<Item = InlineFetch<'_>> {
        self.parts.iter().filter_map(|p| {
            let cid = p.content_id.as_deref()?;
            p.is_inline.then_some(InlineFetch {
                content_id: cid,
                mime_type: &p.mime_type,
                attachment_id: p.attachment_id.as_ref(),
                inline_data: p.inline_data.as_deref(),
            })
        })
    }
}

#[must_use]
pub fn parse_message(root: &WirePart) -> ParsedMessage {
    let headers = extract_headers(&root.headers);
    let mut acc = WalkAcc::default();
    walk_part(root, 0, &mut acc);
    finish_walk(headers, acc)
}

#[derive(Default)]
struct WalkAcc {
    plain_parts: Vec<String>,
    html_parts: Vec<String>,
    own_cids: HashSet<String>,
    nested_cids: Vec<HashSet<String>>,
    candidates: Vec<Candidate>,
}

struct Candidate {
    scope: usize,
    part: Part,
}

fn walk_part(part: &WirePart, scope: usize, acc: &mut WalkAcc) {
    let mime = header_kind(if part.mime_type.is_empty() {
        find_header(&part.headers, "content-type").unwrap_or("text/plain")
    } else {
        &part.mime_type
    });

    if mime.starts_with("multipart/") {
        for child in &part.parts {
            walk_part(child, scope, acc);
        }
        return;
    }

    if mime == "message/rfc822" && !part.parts.is_empty() {
        if part.body.attachment_id.is_some() {
            acc.candidates.push(Candidate {
                scope,
                part: build_part(part, mime, resolve_filename(part)),
            });
        }
        let nested_scope = acc.nested_cids.len() + 1;
        acc.nested_cids.push(HashSet::new());
        for child in &part.parts {
            walk_part(child, nested_scope, acc);
        }
        return;
    }

    let disp_kind = header_kind(find_header(&part.headers, "content-disposition").unwrap_or(""));
    let is_attachment_disp = disp_kind == "attachment";
    let filename = resolve_filename(part);
    let has_att_id = part.body.attachment_id.is_some();

    if !is_attachment_disp
        && filename.is_empty()
        && !has_att_id
        && record_body(part, &mime, scope, acc)
    {
        return;
    }
    if has_att_id
        || !filename.is_empty()
        || is_attachment_disp
        || find_header(&part.headers, "content-id").is_some()
    {
        acc.candidates.push(Candidate {
            scope,
            part: build_part(part, mime, filename),
        });
    }
}

fn record_body(part: &WirePart, mime: &str, scope: usize, acc: &mut WalkAcc) -> bool {
    if mime == "text/plain" {
        if scope == 0
            && let Some(text) = decode_part_text(part)
        {
            acc.plain_parts.push(text);
        }
        return true;
    }
    if mime == "text/html" {
        if let Some(html) = decode_part_text(part) {
            let cids = collect_cid_refs(&html);
            if scope == 0 {
                acc.own_cids.extend(cids);
                acc.html_parts.push(html);
            } else if let Some(set) = acc.nested_cids.get_mut(scope - 1) {
                set.extend(cids);
            }
        }
        return true;
    }
    false
}

fn build_part(part: &WirePart, mime: String, filename: String) -> Part {
    let content_id = find_header(&part.headers, "content-id").and_then(normalize_cid);
    let inline_data = if part.body.attachment_id.is_none() {
        part.body.data.as_deref().map(decode_base64_lenient)
    } else {
        None
    };
    let size = if part.body.size > 0 {
        part.body.size
    } else {
        inline_data
            .as_ref()
            .map_or(0, |d| i64::try_from(d.len()).unwrap_or(i64::MAX))
    };
    let name = if filename.is_empty() {
        default_filename(&mime, &part.part_id)
    } else {
        filename
    };
    Part {
        id: part.part_id.clone(),
        filename: name,
        mime_type: mime,
        size,
        attachment_id: part.body.attachment_id.clone(),
        content_id,
        is_inline: false,
        inline_data,
    }
}

fn finish_walk(headers: MessageHeaders, acc: WalkAcc) -> ParsedMessage {
    let mut parts = Vec::with_capacity(acc.candidates.len());
    for mut cand in acc.candidates {
        let is_image = cand.part.mime_type.starts_with("image/");
        let referenced = cand.part.content_id.as_deref().is_some_and(|cid| {
            if cand.scope == 0 {
                acc.own_cids.contains(cid)
            } else {
                acc.nested_cids
                    .get(cand.scope - 1)
                    .is_some_and(|set| set.contains(cid))
            }
        });
        if cand.scope > 0 && is_image && referenced {
            continue;
        }
        if cand.scope == 0 {
            cand.part.is_inline = is_image && referenced;
        }
        parts.push(cand.part);
    }
    let text = (!acc.plain_parts.is_empty()).then(|| acc.plain_parts.join("\n"));
    let html = (!acc.html_parts.is_empty()).then(|| acc.html_parts.join("\n"));
    ParsedMessage {
        headers,
        raw_body: RawBody { html, text },
        parts,
    }
}

fn extract_headers(headers: &[WireHeader]) -> MessageHeaders {
    let mut out = MessageHeaders::default();
    let mut has_list_hdr = false;
    for h in headers {
        match h.name.to_ascii_lowercase().as_str() {
            "from" => out.from.extend(parse_address_list(&h.value)),
            "to" => out.to.extend(parse_address_list(&h.value)),
            "cc" => out.cc.extend(parse_address_list(&h.value)),
            "bcc" => out.bcc.extend(parse_address_list(&h.value)),
            "reply-to" => out.reply_to.extend(parse_address_list(&h.value)),
            "subject" => out.subject = decode_encoded_words(&h.value).into_owned(),
            "date" => out.date = parse_date(&h.value),
            "message-id" => out.message_id = bare_id(&h.value),
            "in-reply-to" => out.in_reply_to = bare_id(&h.value),
            "references" => out.references.extend(message_ids(&h.value)),
            "list-id" => has_list_hdr = true,
            "list-unsubscribe" => {
                has_list_hdr = true;
                out.list_unsubscribe = Some(h.value.clone());
            }
            "list-unsubscribe-post" => out.list_unsubscribe_post = Some(h.value.clone()),
            "precedence"
                if matches!(
                    h.value.trim().to_ascii_lowercase().as_str(),
                    "bulk" | "list"
                ) =>
            {
                has_list_hdr = true;
            }
            name if name.eq_ignore_ascii_case(DRAFT_HEADER) => {
                out.draft_id = parse_draft_id(&h.value);
            }
            _ => {}
        }
    }
    out.is_list = has_list_hdr;
    out
}

fn find_header<'a>(headers: &'a [WireHeader], target: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(target))
        .map(|h| h.value.as_str())
}

fn resolve_filename(part: &WirePart) -> String {
    if !part.filename.trim().is_empty() {
        return decode_encoded_words(part.filename.trim()).into_owned();
    }
    if let Some(disp) = find_header(&part.headers, "content-disposition")
        && let Some(name) = header_param(disp, "filename")
    {
        return name;
    }
    if let Some(ct) = find_header(&part.headers, "content-type")
        && let Some(name) = header_param(ct, "name")
    {
        return name;
    }
    String::new()
}

fn default_filename(mime: &str, part_id: &str) -> String {
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "application/pdf" => "pdf",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "message/rfc822" => "eml",
        _ => "bin",
    };
    let suffix = if part_id.is_empty() { "file" } else { part_id };
    format!("attachment-{suffix}.{ext}")
}

fn decode_part_text(part: &WirePart) -> Option<String> {
    let raw = part.body.data.as_deref()?;
    let bytes = decode_base64_lenient(raw);
    let charset =
        find_header(&part.headers, "content-type").and_then(|ct| header_param(ct, "charset"));
    Some(decode_text_bytes(&bytes, charset.as_deref()))
}

fn decode_text_bytes(bytes: &[u8], charset: Option<&str>) -> String {
    if bytes.iter().any(|&b| b >= 0x80)
        && let Ok(text) = std::str::from_utf8(bytes)
    {
        return text.to_owned();
    }
    if let Some(label) = charset
        && let Some(enc) = Encoding::for_label(label.trim().as_bytes())
    {
        let (decoded, _, _) = enc.decode(bytes);
        return decoded.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned(),
    }
}

#[must_use]
pub fn decode_base64_lenient(input: &str) -> Vec<u8> {
    let cleaned: String = input.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if let Ok(bytes) = URL_SAFE_NO_PAD.decode(&cleaned) {
        return bytes;
    }
    if let Ok(bytes) = URL_SAFE.decode(&cleaned) {
        return bytes;
    }
    let standard = cleaned.replace('-', "+").replace('_', "/");
    let padded = match standard.len() % 4 {
        2 => format!("{standard}=="),
        3 => format!("{standard}="),
        _ => standard,
    };
    STANDARD.decode(&padded).unwrap_or_default()
}

pub(crate) fn header_kind(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

pub(crate) fn header_param(header: &str, want: &str) -> Option<String> {
    let mut ext_parts: Vec<(u32, String)> = Vec::new();
    let mut ext_single: Option<String> = None;
    let mut plain: Option<String> = None;
    for segment in split_header_params(header).into_iter().skip(1) {
        let Some((raw_key, raw_val)) = segment.split_once('=') else {
            continue;
        };
        let key = raw_key.trim().to_ascii_lowercase();
        let val = unquote_param(raw_val.trim());
        if key == format!("{want}*") {
            ext_single = Some(decode_rfc2231(&val));
        } else if let Some(rest) = key.strip_prefix(&format!("{want}*")) {
            let idx_str = rest.trim_end_matches('*');
            if let Ok(idx) = idx_str.parse::<u32>() {
                ext_parts.push((idx, val.into_owned()));
            }
        } else if key == want {
            plain = Some(decode_encoded_words(&val).into_owned());
        }
    }
    if let Some(v) = ext_single {
        return Some(v);
    }
    if !ext_parts.is_empty() {
        ext_parts.sort_by_key(|(i, _)| *i);
        let joined: String = ext_parts.into_iter().map(|(_, v)| v).collect();
        return Some(decode_rfc2231(&joined));
    }
    plain
}

fn split_header_params(header: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let mut escaped = false;
    for (i, b) in header.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match b {
            b'\\' if in_quotes => escaped = true,
            b'"' => in_quotes = !in_quotes,
            b';' if !in_quotes => {
                out.push(&header[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&header[start..]);
    out
}

fn unquote_param(val: &str) -> Cow<'_, str> {
    if val.len() >= 2 && val.starts_with('"') && val.ends_with('"') {
        let inner = &val[1..val.len() - 1];
        if !inner.contains('\\') {
            return Cow::Borrowed(inner);
        }
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\'
                && let Some(next) = chars.next()
            {
                out.push(next);
            } else {
                out.push(c);
            }
        }
        Cow::Owned(out)
    } else {
        Cow::Borrowed(val)
    }
}

fn decode_rfc2231(val: &str) -> String {
    if let Some((charset, rest)) = val.split_once('\'')
        && let Some((_, encoded)) = rest.split_once('\'')
    {
        let bytes = percent_decode_str(encoded).collect::<Vec<u8>>();
        return decode_text_bytes(&bytes, Some(charset));
    }
    percent_decode_str(val).decode_utf8_lossy().into_owned()
}

pub(crate) fn normalize_cid(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_start_matches('<').trim_end_matches('>');
    let decoded = percent_decode_str(trimmed).decode_utf8_lossy();
    let clean = decoded.trim().to_ascii_lowercase();
    (!clean.is_empty()).then_some(clean)
}

#[must_use]
pub fn decode_encoded_words(input: &str) -> Cow<'_, str> {
    if !input.contains("=?") {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    let mut pending_charset: Option<String> = None;
    let mut pending_bytes: Vec<u8> = Vec::new();
    let mut pending_ws = String::new();

    while let Some(start) = rest.find("=?") {
        let before = &rest[..start];
        let after_prefix = &rest[start..];
        if let Some((charset, bytes, consumed)) = parse_encoded_word(after_prefix) {
            if !before.is_empty() && !before.bytes().all(|b| b.is_ascii_whitespace()) {
                flush_encoded(&mut out, &mut pending_charset, &mut pending_bytes);
                out.push_str(&pending_ws);
                pending_ws.clear();
                out.push_str(before);
            } else if pending_charset.is_none() {
                out.push_str(before);
            } else {
                pending_ws.clear();
            }
            if pending_charset
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case(&charset))
            {
                pending_bytes.extend_from_slice(&bytes);
            } else {
                flush_encoded(&mut out, &mut pending_charset, &mut pending_bytes);
                pending_charset = Some(charset);
                pending_bytes = bytes;
            }
            rest = &after_prefix[consumed..];
        } else {
            flush_encoded(&mut out, &mut pending_charset, &mut pending_bytes);
            out.push_str(&pending_ws);
            pending_ws.clear();
            out.push_str(&rest[..start + 2]);
            rest = &rest[start + 2..];
        }
    }
    flush_encoded(&mut out, &mut pending_charset, &mut pending_bytes);
    out.push_str(rest);
    Cow::Owned(out)
}

fn flush_encoded(out: &mut String, charset: &mut Option<String>, bytes: &mut Vec<u8>) {
    if let Some(cs) = charset.take() {
        out.push_str(&decode_text_bytes(bytes, Some(&cs)));
        bytes.clear();
    }
}

fn parse_encoded_word(s: &str) -> Option<(String, Vec<u8>, usize)> {
    let body = s.strip_prefix("=?")?;
    let q1 = body.find('?')?;
    let charset_raw = &body[..q1];
    let charset = charset_raw.split_once('*').map_or(charset_raw, |(c, _)| c);
    if charset.is_empty() || charset.bytes().any(|b| b.is_ascii_whitespace()) {
        return None;
    }
    let after_cs = &body[q1 + 1..];
    let q2 = after_cs.find('?')?;
    let encoding = &after_cs[..q2];
    if encoding.len() != 1 {
        return None;
    }
    let after_enc = &after_cs[q2 + 1..];
    let end = after_enc.find("?=")?;
    let encoded = &after_enc[..end];
    if encoded.bytes().any(|b| b.is_ascii_whitespace()) {
        return None;
    }
    let bytes = match encoding.as_bytes()[0].to_ascii_uppercase() {
        b'B' => decode_base64_lenient(encoded),
        b'Q' => decode_q_encoding(encoded),
        _ => return None,
    };
    let consumed = 2 + q1 + 1 + q2 + 1 + end + 2;
    Some((charset.to_owned(), bytes, consumed))
}

fn decode_q_encoding(encoded: &str) -> Vec<u8> {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'_' => {
                out.push(b' ');
                i += 1;
            }
            b'=' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(b'=');
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

const fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[must_use]
pub fn parse_address_list(raw: &str) -> Vec<Addr> {
    let items = split_address_items(raw);
    let mut out = Vec::new();
    let mut pending_prefix: Option<String> = None;

    for item in items {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let cleaned = strip_group_prefix(trimmed);
        if let Some(mut addr) = parse_single_address(cleaned) {
            if let Some(prefix) = pending_prefix.take() {
                let merged = match addr.name {
                    Some(name) if !name.is_empty() => format!("{prefix}, {name}"),
                    _ => prefix,
                };
                addr.name = Some(merged);
            }
            out.push(addr);
        } else if !cleaned.contains('@') && !cleaned.contains('<') {
            pending_prefix = Some(match pending_prefix {
                Some(prev) => format!("{prev}, {cleaned}"),
                None => cleaned.to_owned(),
            });
        } else {
            pending_prefix = None;
        }
    }
    out
}

fn split_address_items(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut escaped = false;
    let mut comment_depth = 0u32;
    let mut angle_depth = 0u32;

    for ch in raw.chars() {
        if escaped {
            cur.push(ch);
            escaped = false;
            continue;
        }
        if in_quotes {
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_quotes = false;
            }
            cur.push(ch);
            continue;
        }
        if comment_depth > 0 {
            if ch == '(' {
                comment_depth += 1;
            } else if ch == ')' {
                comment_depth -= 1;
            }
            continue;
        }
        match ch {
            '"' => {
                in_quotes = true;
                cur.push(ch);
            }
            '(' => comment_depth += 1,
            '<' => {
                angle_depth += 1;
                cur.push(ch);
            }
            '>' => {
                angle_depth = angle_depth.saturating_sub(1);
                cur.push(ch);
            }
            ',' | ';' if angle_depth == 0 => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn strip_group_prefix(s: &str) -> &str {
    if let Some(colon) = s.find(':') {
        let before = &s[..colon];
        if !before.contains('<') && !before.contains('@') && !before.contains('"') {
            return s[colon + 1..].trim();
        }
    }
    s
}

fn parse_single_address(raw: &str) -> Option<Addr> {
    if let Some(open) = raw.rfind('<')
        && let Some(close_rel) = raw[open + 1..].find('>')
    {
        let email = raw[open + 1..open + 1 + close_rel].trim();
        if email.is_empty() || !email.contains('@') {
            return None;
        }
        let name_part = raw[..open].trim();
        let name = clean_display_name(name_part);
        return Some(Addr {
            name,
            email: email.to_owned(),
        });
    }
    let candidate = raw.trim();
    if candidate.contains('@') && !candidate.chars().any(char::is_whitespace) {
        return Some(Addr::new(candidate));
    }
    None
}

fn clean_display_name(raw: &str) -> Option<String> {
    let unquoted = unquote_param(raw.trim());
    let decoded = decode_encoded_words(&unquoted);
    let trimmed = decoded.trim().trim_matches('"').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[must_use]
pub fn is_valid_email(email: &str) -> bool {
    if email.is_empty() || email.len() > 254 {
        return false;
    }
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    if domain.contains('@') || !valid_local(local) || !valid_domain(domain) {
        return false;
    }
    true
}

fn valid_local(local: &str) -> bool {
    if local.is_empty() || local.len() > 64 || local.starts_with('.') || local.ends_with('.') {
        return false;
    }
    let mut prev_dot = false;
    for b in local.bytes() {
        let ok = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'!' | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'/'
                    | b'='
                    | b'?'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'{'
                    | b'|'
                    | b'}'
                    | b'~'
                    | b'.'
            );
        if !ok || (b == b'.' && prev_dot) {
            return false;
        }
        prev_dot = b == b'.';
    }
    true
}

fn valid_domain(domain: &str) -> bool {
    if domain.is_empty() || domain.starts_with('.') || domain.ends_with('.') {
        return false;
    }
    let mut count = 0usize;
    let mut last = "";
    for label in domain.split('.') {
        if label.is_empty() || label.len() > 63 || label.starts_with('-') || label.ends_with('-') {
            return false;
        }
        if !label.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '-' || (!c.is_ascii() && c.is_alphanumeric())
        }) {
            return false;
        }
        count += 1;
        last = label;
    }
    count >= 2 && last.chars().count() >= 2 && !last.bytes().all(|b| b.is_ascii_digit())
}

pub(crate) fn bare_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if let Some(start) = trimmed.find('<')
        && let Some(end) = trimmed[start + 1..].find('>')
    {
        let inner = trimmed[start + 1..start + 1 + end].trim();
        return (!inner.is_empty()).then(|| format!("<{inner}>"));
    }
    let clean = trimmed.split_whitespace().next()?;
    (!clean.is_empty() && clean.contains('@')).then(|| format!("<{clean}>"))
}

pub(crate) fn message_ids(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find('<') {
        let tail = &rest[start + 1..];
        let Some(end) = tail.find('>') else {
            break;
        };
        let inner = tail[..end].trim();
        if !inner.is_empty() {
            out.push(format!("<{inner}>"));
        }
        rest = &tail[end + 1..];
    }
    if out.is_empty() {
        for tok in raw.split_whitespace() {
            if tok.contains('@') {
                out.push(format!("<{}>", tok.trim_matches(|c| c == '<' || c == '>')));
            }
        }
    }
    out
}

pub(crate) fn is_message_id(raw: &str) -> bool {
    let s = raw.trim();
    !s.is_empty()
        && !s.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
        && s.starts_with('<')
        && s.ends_with('>')
        && s.len() >= 3
        && !s[1..s.len() - 1].chars().any(char::is_whitespace)
}

pub(crate) fn is_draft_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn parse_draft_id(raw: &str) -> Option<DraftId> {
    let trimmed = raw.trim();
    is_draft_token(trimmed).then(|| DraftId(trimmed.to_owned()))
}

fn parse_date(raw: &str) -> Option<Millis> {
    let trimmed = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc2822(trimmed) {
        return Some(Millis(dt.timestamp_millis()));
    }
    let no_comment = strip_date_comment(trimmed);
    DateTime::parse_from_rfc2822(no_comment.trim())
        .ok()
        .map(|dt| Millis(dt.timestamp_millis()))
}

fn strip_date_comment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    for ch in s.chars() {
        if ch == '(' {
            depth += 1;
        } else if ch == ')' && depth > 0 {
            depth -= 1;
        } else if depth == 0 {
            out.push(ch);
        }
    }
    out
}
