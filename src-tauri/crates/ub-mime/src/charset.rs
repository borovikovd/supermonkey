//! Bytes to text: declared charsets, and base64 as mail producers really write it.

use std::borrow::Cow;

use base64::Engine as _;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use encoding_rs::{Encoding, WINDOWS_1252};

/// Accepts input with or without `=` padding, and stray bits in the last byte.
const LENIENT: GeneralPurposeConfig = GeneralPurposeConfig::new()
    .with_decode_padding_mode(DecodePaddingMode::Indifferent)
    .with_decode_allow_trailing_bits(true);
const URL_SAFE: GeneralPurpose = GeneralPurpose::new(&alphabet::URL_SAFE, LENIENT);
const STANDARD: GeneralPurpose = GeneralPurpose::new(&alphabet::STANDARD, LENIENT);

/// Decodes base64 in either alphabet, padded or not. Gmail's `body.data` is
/// base64url; `data:` URIs and RFC 2047 `B` words use the standard alphabet; and
/// some producers mix them or wrap lines. The common cases decode without a copy.
pub(crate) fn decode_base64(data: &str) -> Option<Vec<u8>> {
    if let Ok(bytes) = URL_SAFE.decode(data) {
        return Some(bytes);
    }
    if let Ok(bytes) = STANDARD.decode(data) {
        return Some(bytes);
    }
    let normalized: String = data
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '=')
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    URL_SAFE.decode(normalized).ok()
}

/// Standard, padded base64, as `data:` URIs carry it.
pub(crate) fn encode_base64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Decodes `bytes` in the declared charset. With no charset, or one nobody knows,
/// the bytes are read as UTF-8, and as windows-1252 when they are not valid UTF-8:
/// undeclared 8-bit mail is nearly always Latin-1 in practice. A known charset
/// replaces invalid sequences with U+FFFD.
pub(crate) fn decode_charset(bytes: Vec<u8>, charset: Option<&str>) -> String {
    if let Some(encoding) = charset.and_then(|c| Encoding::for_label(label(c).as_bytes())) {
        // Borrowed means the bytes already are this text in UTF-8.
        if let Cow::Owned(decoded) = encoding.decode_without_bom_handling(&bytes).0 {
            return decoded;
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|err| {
        WINDOWS_1252
            .decode_without_bom_handling(err.as_bytes())
            .0
            .into_owned()
    })
}

/// A charset name without quotes or an RFC 2231 language suffix (`utf-8*en`).
fn label(charset: &str) -> &str {
    let label = charset.trim().trim_matches('"');
    label.split_once('*').map_or(label, |(name, _)| name)
}
