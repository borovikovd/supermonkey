//! Byte and header decoding: charsets, base64url bodies, RFC 2047 encoded-words.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};

/// Decodes `bytes` in the named charset. Unknown or missing charsets fall back to UTF-8,
/// then to windows-1252 when the bytes are not valid UTF-8.
pub(crate) fn decode_charset(bytes: &[u8], charset: Option<&str>) -> String {
    let label = charset.map_or("", |c| c.trim().trim_matches('"'));
    // RFC 2231 allows a language suffix: `utf-8*en`.
    let label = label.split('*').next().unwrap_or(label);
    if let Some(enc) = encoding_rs::Encoding::for_label(label.as_bytes()) {
        return enc.decode_without_bom_handling(bytes).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1252
            .decode_without_bom_handling(bytes)
            .0
            .into_owned(),
    }
}

/// Decodes Gmail's base64url `body.data`. Also accepts padding, whitespace and the
/// standard alphabet, since other producers are not always strict.
pub(crate) fn decode_base64url(data: &str) -> Option<Vec<u8>> {
    let cleaned: String = data
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    URL_SAFE_NO_PAD.decode(cleaned).ok()
}

/// Decodes standard base64 with optional whitespace (as found in `data:` URIs).
pub(crate) fn decode_base64(data: &str) -> Option<Vec<u8>> {
    let cleaned: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    STANDARD.decode(cleaned).ok()
}

pub(crate) fn encode_base64(data: &[u8]) -> String {
    STANDARD.encode(data)
}

/// Decodes RFC 2047 encoded-words (`=?UTF-8?B?...?=`, `=?iso-8859-1?Q?...?=`) in a header value.
/// Whitespace between adjacent encoded-words is dropped, and adjacent words in the same charset
/// are decoded together so multi-byte characters split across words survive.
/// Malformed words are left as they are.
pub fn decode_encoded_words(input: &str) -> String {
    if !input.contains("=?") {
        return input.to_string();
    }
    let mut out = String::new();
    // Pending bytes of adjacent encoded-words in one charset, decoded when the run ends.
    let mut pending: Option<(String, Vec<u8>)> = None;
    // Whitespace seen after an encoded-word; dropped if another encoded-word follows.
    let mut gap = String::new();
    let mut rest = input;

    while !rest.is_empty() {
        if let Some((charset, bytes, len)) = rest.strip_prefix("=?").and_then(parse_word) {
            gap.clear();
            match &mut pending {
                Some((cs, buf)) if cs.eq_ignore_ascii_case(&charset) => buf.extend(bytes),
                _ => {
                    flush(&mut out, &mut pending);
                    pending = Some((charset, bytes));
                }
            }
            rest = &rest[len + 2..];
            continue;
        }
        let ch = rest.chars().next().unwrap_or(' ');
        if pending.is_some() && ch.is_whitespace() {
            gap.push(ch);
        } else {
            flush(&mut out, &mut pending);
            out.push_str(&gap);
            gap.clear();
            out.push(ch);
        }
        rest = &rest[ch.len_utf8()..];
    }
    flush(&mut out, &mut pending);
    out.push_str(&gap);
    out
}

fn flush(out: &mut String, pending: &mut Option<(String, Vec<u8>)>) {
    if let Some((charset, bytes)) = pending.take() {
        out.push_str(&decode_charset(&bytes, Some(&charset)));
    }
}

/// Parses `charset?E?text?=` (the part after `=?`). Returns the charset, decoded bytes,
/// and the number of bytes consumed.
fn parse_word(s: &str) -> Option<(String, Vec<u8>, usize)> {
    let mut fields = s.splitn(3, '?');
    let charset = fields.next()?;
    let encoding = fields.next()?;
    let tail = fields.next()?;
    let end = tail.find("?=")?;
    let text = &tail[..end];
    if charset.is_empty() || text.contains(char::is_whitespace) {
        return None;
    }
    let bytes = match encoding {
        "B" | "b" => {
            decode_base64(text.trim_end_matches('=')).or_else(|| decode_base64url(text))?
        }
        "Q" | "q" => decode_q(text),
        _ => return None,
    };
    let consumed = charset.len() + 1 + encoding.len() + 1 + end + 2;
    Some((charset.to_string(), bytes, consumed))
}

fn decode_q(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'_' => out.push(b' '),
            b'=' => match text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                Some(b) => {
                    out.push(b);
                    i += 2;
                }
                None => out.push(b'='),
            },
            b => out.push(b),
        }
        i += 1;
    }
    out
}

/// Extracts a parameter (`charset`, `name`, `filename`) from a structured header value such as
/// `text/html; charset="utf-8"`. Handles quoted values and RFC 2231 `name*=utf-8''...` forms,
/// including `name*0*=` continuations.
pub(crate) fn header_param(value: &str, param: &str) -> Option<String> {
    let mut plain = None;
    let mut extended: Vec<(u32, bool, String)> = Vec::new();
    for raw in split_params(value).into_iter().skip(1) {
        let Some((key, val)) = raw.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let val = val.trim();
        let unquoted = unquote(val);
        if key == param {
            plain = Some(unquoted);
            continue;
        }
        let Some(rest) = key.strip_prefix(param).and_then(|r| r.strip_prefix('*')) else {
            continue;
        };
        // `name*`, `name*0`, `name*0*`
        let encoded = rest.is_empty() || rest.ends_with('*');
        let index = match rest.trim_end_matches('*') {
            "" => 0,
            n => match n.parse::<u32>() {
                Ok(i) => i,
                Err(_) => continue,
            },
        };
        extended.push((index, encoded, unquoted));
    }
    if extended.is_empty() {
        return plain.map(|p| decode_encoded_words(&p));
    }
    extended.sort_by_key(|(i, _, _)| *i);
    let mut charset = None;
    let mut bytes = Vec::new();
    for (i, encoded, val) in extended {
        if !encoded {
            bytes.extend(val.as_bytes());
            continue;
        }
        let mut val = val.as_str();
        if i == 0 {
            // charset'language'value
            let mut it = val.splitn(3, '\'');
            if let (Some(cs), Some(_), Some(v)) = (it.next(), it.next(), it.next()) {
                charset = Some(cs.to_string());
                val = v;
            }
        }
        bytes.extend(percent_decode(val));
    }
    Some(decode_charset(&bytes, charset.as_deref()))
}

/// Splits a structured header on `;` outside quotes.
fn split_params(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut in_quote = false;
    let mut start = 0;
    let mut escaped = false;
    for (i, c) in value.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if in_quote => escaped = true,
            '"' => in_quote = !in_quote,
            ';' if !in_quote => {
                out.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&value[start..]);
    out
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    match s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        Some(inner) => {
            let mut out = String::with_capacity(inner.len());
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(n) = chars.next() {
                        out.push(n);
                    }
                } else {
                    out.push(c);
                }
            }
            out
        }
        None => s.to_string(),
    }
}

/// Decodes `%XX` escapes; invalid escapes are kept literally.
pub(crate) fn percent_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_b_and_q_words() {
        assert_eq!(
            decode_encoded_words("=?UTF-8?B?SGVsbG8gV29ybGQ=?="),
            "Hello World"
        );
        assert_eq!(
            decode_encoded_words("=?iso-8859-1?Q?Andr=E9_Pirard?="),
            "André Pirard"
        );
        assert_eq!(decode_encoded_words("plain text"), "plain text");
    }

    #[test]
    fn joins_adjacent_words_and_keeps_surrounding_text() {
        assert_eq!(
            decode_encoded_words("Re: =?UTF-8?Q?caf=C3?= =?UTF-8?Q?=A9?= time"),
            "Re: café time"
        );
        assert_eq!(decode_encoded_words("=?UTF-8?Q?a?= b"), "a b");
    }

    #[test]
    fn leaves_malformed_words_alone() {
        assert_eq!(decode_encoded_words("=?UTF-8?X?abc?="), "=?UTF-8?X?abc?=");
        assert_eq!(decode_encoded_words("=?broken"), "=?broken");
        assert_eq!(decode_encoded_words("a =? b"), "a =? b");
    }

    #[test]
    fn decodes_charsets() {
        assert_eq!(
            decode_charset(&[0x63, 0x61, 0x66, 0xE9], Some("ISO-8859-1")),
            "café"
        );
        assert_eq!(
            decode_charset(&[0x93, 0x68, 0x69, 0x94], Some("windows-1252")),
            "\u{201c}hi\u{201d}"
        );
        assert_eq!(decode_charset("é".as_bytes(), None), "é");
        assert_eq!(decode_charset(&[0xE9], Some("x-unknown")), "é");
    }

    #[test]
    fn decodes_base64url_variants() {
        assert_eq!(decode_base64url("aGk_Pz8-").unwrap(), b"hi???>");
        assert_eq!(decode_base64url("aGk/Pz8+").unwrap(), b"hi???>");
        assert_eq!(decode_base64url("aGk=\n").unwrap(), b"hi");
    }

    #[test]
    fn reads_header_params() {
        assert_eq!(
            header_param("text/html; charset=\"ISO-8859-1\"", "charset").as_deref(),
            Some("ISO-8859-1")
        );
        assert_eq!(
            header_param(
                "attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf",
                "filename"
            )
            .as_deref(),
            Some("résumé.pdf")
        );
        assert_eq!(
            header_param(
                "attachment; filename*0*=UTF-8''a%C3%A9; filename*1=\"b c.txt\"",
                "filename"
            )
            .as_deref(),
            Some("aéb c.txt")
        );
        assert_eq!(
            header_param("image/png; name=\"=?UTF-8?B?w6kucG5n?=\"", "name").as_deref(),
            Some("é.png")
        );
        assert_eq!(header_param("text/plain", "charset"), None);
    }
}
