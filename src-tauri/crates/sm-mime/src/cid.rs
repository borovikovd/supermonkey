//! Inline images: `cid:` references to `data:` URIs when reading, and back
//! when sending (SPEC Reading, Reply composer).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use percent_encoding::percent_decode_str;
use regex::{Captures, Regex};

use crate::charset::{decode_base64, encode_base64};
use crate::headers::{bare_id, is_message_id};
use crate::outgoing::InlinePart;

/// The inline images of one message by Content-ID, as `data:` URIs, for
/// rewriting its `cid:` references before sanitizing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CidImages {
    by_content_id: HashMap<String, String>,
}

impl CidImages {
    /// No images.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the bytes of the part with this Content-ID (brackets optional). The
    /// first image added for a Content-ID is kept.
    pub fn insert(&mut self, content_id: &str, mime_type: &str, bytes: &[u8]) {
        self.by_content_id
            .entry(bare_id(content_id).to_owned())
            .or_insert_with(|| format!("data:{mime_type};base64,{}", encode_base64(bytes)));
    }

    /// Whether there are no images.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_content_id.is_empty()
    }

    /// The `data:` URI for a `cid:` reference as written in HTML, which may be
    /// percent-encoded or bracketed.
    fn uri_for(&self, reference: &str) -> Option<&str> {
        if let Some(uri) = self.by_content_id.get(reference) {
            return Some(uri);
        }
        let decoded = percent_decode_str(reference).decode_utf8_lossy();
        self.by_content_id
            .get(bare_id(&decoded))
            .map(String::as_str)
    }
}

static CID_REF: LazyLock<Regex> = LazyLock::new(|| crate::regex(r#"(?i)\bcid:([^"'\s)>]+)"#));

/// The Content-IDs (without brackets) that `html`'s `cid:` references name,
/// read as [`CidImages`] reads them: as written, and percent-decoded
/// without brackets.
pub(crate) fn cid_references(html: &str) -> impl Iterator<Item = String> + '_ {
    CID_REF.captures_iter(html).flat_map(|c| {
        let reference = c.get(1).map_or("", |m| m.as_str());
        let decoded = percent_decode_str(reference).decode_utf8_lossy();
        [reference.to_owned(), bare_id(&decoded).to_owned()]
    })
}

/// Replaces each `cid:` reference with a known image by its `data:` URI.
/// Unknown references stay; the sanitizer then drops them.
pub(crate) fn inline_cid_images<'a>(html: &'a str, images: &CidImages) -> Cow<'a, str> {
    if images.is_empty() {
        return Cow::Borrowed(html);
    }
    CID_REF.replace_all(html, |c: &Captures<'_>| {
        let whole = c.get(0).map_or("", |m| m.as_str());
        let reference = c.get(1).map_or("", |m| m.as_str());
        images.uri_for(reference).unwrap_or(whole).to_owned()
    })
}

static DATA_URI: LazyLock<Regex> =
    LazyLock::new(|| crate::regex(r"(?i)data:[a-z]+/[a-z0-9.+-]+;base64,([a-z0-9+/=]+)"));

/// Turns `data:` images in outgoing HTML back into `cid:` references when their
/// bytes equal one of `known`, the inline parts of the quoted or forwarded
/// message: Gmail does not show `data:` images (SPEC Reply composer).
///
/// Returns the HTML and the parts it now references, in order of first use and
/// without duplicates; the rest of `known` is dropped, as are parts whose
/// Content-ID could not be written into a header safely. Other `data:` URIs stay.
pub fn data_images_to_cid(
    html: &str,
    mut known: Vec<InlinePart>,
) -> (Cow<'_, str>, Vec<InlinePart>) {
    known.retain(|part| is_message_id(bare_id(&part.content_id)));
    let mut used: Vec<InlinePart> = Vec::new();
    let rewritten = DATA_URI.replace_all(html, |c: &Captures<'_>| {
        let whole = c.get(0).map_or("", |m| m.as_str());
        let Some(bytes) = c.get(1).and_then(|m| decode_base64(m.as_str())) else {
            return whole.to_owned();
        };
        // A part moves to `used` on first use, so a repeat finds it there.
        if let Some(part) = used.iter().find(|p| p.data == bytes) {
            return format!("cid:{}", bare_id(&part.content_id));
        }
        let Some(i) = known.iter().position(|p| p.data == bytes) else {
            return whole.to_owned();
        };
        let part = known.remove(i);
        let reference = format!("cid:{}", bare_id(&part.content_id));
        if !used.iter().any(|u| u.content_id == part.content_id) {
            used.push(part);
        }
        reference
    });
    (rewritten, used)
}
