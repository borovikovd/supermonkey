use std::borrow::Cow;

use percent_encoding::percent_decode_str;

use crate::charset::decode_charset;
use crate::encoded_word::decode_encoded_words;

pub(crate) fn header_kind(value: &str) -> &str {
    split_params(value).next().map_or("", str::trim)
}

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

fn split_params(value: &str) -> impl Iterator<Item = &str> {
    let mut rest = Some(value);
    std::iter::from_fn(move || {
        let part = rest.take()?;
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
