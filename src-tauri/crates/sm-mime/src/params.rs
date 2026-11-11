//! Parameters of structured headers: `text/html; charset="utf-8"`,
//! `attachment; filename*=utf-8''r%C3%A9sum%C3%A9.pdf`.

use std::borrow::Cow;

use percent_encoding::percent_decode_str;

use crate::charset::decode_charset;
use crate::encoded_word::decode_encoded_words;

/// The kind of a structured header: the value before its first `;`, in the
/// case it was written in (`attachment`, `Text/HTML`). Empty when the header is.
pub(crate) fn header_kind(value: &str) -> &str {
    split_params(value).next().map_or("", str::trim)
}

/// A parameter of a structured header, by case-insensitive name.
///
/// Handles quoted values with escapes, RFC 2047 words in a plain value (common,
/// though not allowed), and RFC 2231 extended values: `name*=charset'lang'%XX`,
/// split over `name*0*=`, `name*1=` sections. Extended values win over plain ones.
pub(crate) fn header_param<'a>(value: &'a str, name: &str) -> Option<Cow<'a, str>> {
    let mut plain = None;
    let mut sections = Vec::new();
    for param in split_params(value).skip(1) {
        let Some((key, val)) = param.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let val = unquote(val);
        if key.eq_ignore_ascii_case(name) {
            plain = Some(val);
        } else if let Some(section) = Section::parse(key, name, val) {
            sections.push(section);
        }
    }
    if sections.is_empty() {
        let plain = plain?;
        let decoded = match decode_encoded_words(&plain) {
            Cow::Owned(decoded) => Some(decoded),
            Cow::Borrowed(_) => None,
        };
        return Some(decoded.map_or(plain, Cow::Owned));
    }
    sections.sort_by_key(|s| s.index);
    let mut charset = None;
    let mut bytes = Vec::new();
    for section in &sections {
        if !section.is_encoded {
            bytes.extend_from_slice(section.value.as_bytes());
            continue;
        }
        let mut encoded = section.value.as_ref();
        if section.index == 0 {
            let mut fields = encoded.splitn(3, '\'');
            if let (Some(cs), Some(_language), Some(rest)) =
                (fields.next(), fields.next(), fields.next())
            {
                charset = Some(cs);
                encoded = rest;
            }
        }
        bytes.extend(percent_decode_str(encoded));
    }
    Some(Cow::Owned(decode_charset(bytes, charset)))
}

/// One RFC 2231 section: `name*` (encoded), `name*0` (plain), `name*0*` (encoded).
struct Section<'a> {
    index: u32,
    is_encoded: bool,
    value: Cow<'a, str>,
}

impl<'a> Section<'a> {
    fn parse(key: &str, name: &str, value: Cow<'a, str>) -> Option<Self> {
        let prefix = key.get(..name.len())?;
        if !prefix.eq_ignore_ascii_case(name) {
            return None;
        }
        let suffix = key.get(name.len()..)?.strip_prefix('*')?;
        let is_encoded = suffix.is_empty() || suffix.ends_with('*');
        let index = match suffix.trim_end_matches('*') {
            "" => 0,
            n => n.parse().ok()?,
        };
        Some(Self {
            index,
            is_encoded,
            value,
        })
    }
}

/// Splits a structured header on the `;`s outside quoted strings: always at
/// least one part, the last being what follows the last such `;`.
fn split_params(value: &str) -> impl Iterator<Item = &str> {
    let mut rest = Some(value);
    std::iter::from_fn(move || {
        let part = rest.take()?;
        // A part ends outside any quote and escape, so the next starts so.
        let mut in_quote = false;
        let mut escaped = false;
        for (i, c) in part.char_indices() {
            match c {
                _ if escaped => escaped = false,
                '\\' if in_quote => escaped = true,
                '"' => in_quote = !in_quote,
                ';' if !in_quote => {
                    rest = Some(part.get(i + 1..).unwrap_or_default());
                    return Some(part.get(..i).unwrap_or_default());
                }
                _ => {}
            }
        }
        Some(part)
    })
}

/// A parameter value without its quotes and backslash escapes.
fn unquote(value: &str) -> Cow<'_, str> {
    let value = value.trim();
    let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
        return Cow::Borrowed(value);
    };
    if !inner.contains('\\') {
        return Cow::Borrowed(inner);
    }
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.extend(chars.next());
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}
