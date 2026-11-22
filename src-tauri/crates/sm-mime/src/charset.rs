use std::borrow::Cow;

use base64::Engine as _;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use encoding_rs::{Encoding, REPLACEMENT, WINDOWS_1252};

const LENIENT: GeneralPurposeConfig = GeneralPurposeConfig::new()
    .with_decode_padding_mode(DecodePaddingMode::Indifferent)
    .with_decode_allow_trailing_bits(true);
const URL_SAFE: GeneralPurpose = GeneralPurpose::new(&alphabet::URL_SAFE, LENIENT);
const STANDARD: GeneralPurpose = GeneralPurpose::new(&alphabet::STANDARD, LENIENT);

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
            other => other,
        })
        .collect();
    URL_SAFE.decode(normalized).ok()
}

pub(crate) fn encode_base64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub(crate) fn decode_charset(bytes: Vec<u8>, charset: Option<&str>) -> String {
    if let Some(encoding) = charset
        .and_then(|c| Encoding::for_label(label(c).as_bytes()))
        .filter(|&e| e != REPLACEMENT)
        && let Cow::Owned(decoded) = encoding.decode_without_bom_handling(&bytes).0
    {
        return decoded;
    }
    String::from_utf8(bytes).unwrap_or_else(|err| {
        WINDOWS_1252
            .decode_without_bom_handling(err.as_bytes())
            .0
            .into_owned()
    })
}

pub(crate) fn decode_text_part(bytes: Vec<u8>, charset: Option<&str>) -> String {
    if !bytes.is_ascii() {
        match String::from_utf8(bytes) {
            Ok(text) => return text,
            Err(err) => return decode_charset(err.into_bytes(), charset),
        }
    }
    decode_charset(bytes, charset)
}

fn label(charset: &str) -> &str {
    let label = charset.trim().trim_matches('"');
    label.split_once('*').map_or(label, |(name, _)| name)
}
