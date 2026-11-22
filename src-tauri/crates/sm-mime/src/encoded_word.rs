use std::borrow::Cow;

use crate::charset::{decode_base64, decode_charset};

pub fn decode_encoded_words(input: &str) -> Cow<'_, str> {
    if !input.contains("=?") {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len());
    let mut run: Option<(&str, Vec<u8>)> = None;
    let mut rest = input;
    while let Some(start) = rest.find("=?") {
        let (gap, at) = rest.split_at(start);
        let Some(word) = EncodedWord::parse(at) else {
            flush(&mut out, &mut run);
            out.push_str(gap);
            out.push_str("=?");
            rest = at.get(2..).unwrap_or_default();
            continue;
        };
        let follows_a_word = run.is_some() && gap.chars().all(char::is_whitespace);
        if !follows_a_word {
            flush(&mut out, &mut run);
            out.push_str(gap);
        }
        match &mut run {
            Some((charset, bytes)) if charset.eq_ignore_ascii_case(word.charset) => {
                bytes.extend(word.bytes);
            }
            _ => {
                flush(&mut out, &mut run);
                run = Some((word.charset, word.bytes));
            }
        }
        rest = at.get(word.len..).unwrap_or_default();
    }
    flush(&mut out, &mut run);
    out.push_str(rest);
    Cow::Owned(out)
}

fn flush(out: &mut String, run: &mut Option<(&str, Vec<u8>)>) {
    if let Some((charset, bytes)) = run.take() {
        out.push_str(&decode_charset(bytes, Some(charset)));
    }
}

struct EncodedWord<'a> {
    charset: &'a str,
    bytes: Vec<u8>,
    len: usize,
}

impl<'a> EncodedWord<'a> {
    fn parse(s: &'a str) -> Option<Self> {
        let inner = s.strip_prefix("=?")?;
        let (charset, after_charset) = inner.split_once('?')?;
        let (encoding, after_encoding) = after_charset.split_once('?')?;
        let end = after_encoding.find("?=")?;
        let text = after_encoding.get(..end)?;
        if charset.is_empty() || charset.contains(char::is_whitespace) {
            return None;
        }
        if text.contains(char::is_whitespace) {
            return None;
        }
        let bytes = match encoding {
            "B" | "b" => decode_base64(text)?,
            "Q" | "q" => decode_q(text),
            _ => return None,
        };
        let len = s.len() - after_encoding.len() + end + "?=".len();
        Some(Self {
            charset,
            bytes,
            len,
        })
    }
}

fn decode_q(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&b, tail)) = rest.split_first() {
        rest = tail;
        match b {
            b'_' => out.push(b' '),
            b'=' => match rest.get(..2).and_then(hex_byte) {
                Some(byte) => {
                    out.push(byte);
                    rest = rest.get(2..).unwrap_or_default();
                }
                None => out.push(b'='),
            },
            _ => out.push(b),
        }
    }
    out
}

fn hex_byte(pair: &[u8]) -> Option<u8> {
    let [hi, lo] = pair else { return None };
    let digit = |d: u8| {
        char::from(d)
            .to_digit(16)
            .and_then(|v| u8::try_from(v).ok())
    };
    Some(digit(*hi)? << 4 | digit(*lo)?)
}
