use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use percent_encoding::percent_decode_str;
use regex::{Captures, Regex};

use crate::charset::{decode_base64, encode_base64};
use crate::headers::{bare_id, is_message_id};
use crate::outgoing::InlinePart;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CidImages {
    by_content_id: HashMap<String, String>,
}

impl CidImages {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, content_id: &str, mime_type: &str, bytes: &[u8]) {
        self.by_content_id
            .entry(bare_id(content_id).to_owned())
            .or_insert_with(|| format!("data:{mime_type};base64,{}", encode_base64(bytes)));
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_content_id.is_empty()
    }

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

pub(crate) fn cid_references(html: &str) -> impl Iterator<Item = String> + '_ {
    CID_REF.captures_iter(html).flat_map(|c| {
        let reference = c.get(1).map_or("", |m| m.as_str());
        let decoded = percent_decode_str(reference).decode_utf8_lossy();
        [reference.to_owned(), bare_id(&decoded).to_owned()]
    })
}

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
