//! # ub-mime
//!
//! Pure MIME and HTML work for Ubermensch: no I/O, no database.
//!
//! ## Addresses
//! - [`Addr`] `{ name: Option<String>, email: String }` (serde, `Addr::new`, `Addr::named`).
//! - [`parse_address_list`]`(header) -> Vec<Addr>`: RFC 5322 lists with quoted names, comments,
//!   RFC 2047 encoded-words, groups, and real-world mess. Never panics; drops entries without `@`.
//! - [`is_valid_email`]`(s) -> bool`: the check for recipient chips.
//! - [`format_addr`]`(&Addr) -> String` / [`format_addr_list`]: `Name <email>`, quoted when needed.
//!
//! ## Received mail
//! - [`parse_gmail_payload`]`(&payload) -> `[`ParsedMessage`]: headers, decoded bodies, and
//!   [`PartInfo`] for attachments and inline parts. Works on `format=full` and `format=metadata`.
//!   Message ids (`message_id`, `in_reply_to`, `references`) are bare, without `<>`.
//! - [`sanitize_html`]`(html, &cid_data)`: rewrites known `cid:` references to `data:` URIs
//!   (build the map with [`data_uri`]), then cleans with ammonia. Store the result.
//! - [`html_to_text`]`(html)`: text for FTS and snippets.
//! - [`text_to_html`]`(text)`: plain-text mail to HTML (escaped, `<br>`, links). Run the result
//!   through [`sanitize_html`] like any other body.
//! - [`split_quoted_history`]`(html) -> (visible, Option<quoted>)`: for the `•••` toggle.
//!
//! ## Compose
//! - [`split_gmail_quote`]`(html) -> (editor_html, Option<quote_html>)`: for drafts read from Gmail.
//! - [`reply_quote_html`] / [`forward_quote_html`]`(&`[`QuoteSource`]`, tz_offset_minutes)`:
//!   Gmail-format quote blocks. `tz_offset_minutes` is minutes east of UTC.
//! - [`reply_subject`] / [`forward_subject`]: add `Re:` / `Fwd:` once.
//! - [`data_images_to_cid`]`(html, known) -> (html, used)`: turns `data:` images that came from
//!   known [`InlinePart`]s back into `cid:` references before building.
//! - [`build_mime`]`(&`[`Outgoing`]`) -> Vec<u8>`: RFC 5322 bytes for `drafts.create/update`
//!   (base64url-encode them into `message.raw`). Writes `X-Ubermensch-Draft`, `In-Reply-To`,
//!   `References`, and a `Message-ID` of `ub.<draft id>.<date_ms>@<message_id_domain>`.
//!
//! A typical send: `html = editor_html + quote_html`, then
//! `let (html, inline) = data_images_to_cid(&html, original_inline_parts)`, then
//! `build_mime(&Outgoing { html, inline, .. })`.
//!
//! [`decode_encoded_words`] is also exported for other headers (e.g. attachment names).

/// Compiles one of this crate's fixed patterns.
#[expect(
    clippy::expect_used,
    reason = "every pattern is a constant; a bad one fails every test that parses mail"
)]
fn regex(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("valid regex")
}

mod address;
mod build;
mod encoding;
mod gmail;
mod html;
mod quote;
mod unsubscribe;

pub use address::{Addr, format_addr, format_addr_list, is_valid_email, parse_address_list};
pub use build::{InlinePart, OutAttachment, Outgoing, build_mime, build_plain};
pub use encoding::decode_encoded_words;
pub use gmail::{ParsedMessage, PartInfo, parse_gmail_payload};
pub use html::{data_images_to_cid, data_uri, html_to_text, sanitize_html, text_to_html};
pub use quote::{
    QuoteSource, forward_quote_html, forward_subject, reply_quote_html, reply_subject,
    split_gmail_quote, split_quoted_history,
};
pub use unsubscribe::{ListUnsubscribe, Mailto, list_unsubscribe};

/// `s` with every run of whitespace turned into one space and the ends trimmed.
pub(crate) fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}
