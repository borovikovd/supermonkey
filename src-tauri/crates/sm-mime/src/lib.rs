//! Mail formats: parse Gmail payloads, sanitize HTML, build outgoing MIME.
//!
//! Pure: no I/O, no clock, no database (ARCHITECTURE.md, section 3).
//!
//! # Received mail
//!
//! [`parse_message`] reads a Gmail `payload` into [`MessageHeaders`], a
//! [`RawBody`], and [`Part`]s. To store a body:
//!
//! 1. start from [`ParsedMessage::cid_images`];
//! 2. fetch each of [`ParsedMessage::inline_images_to_fetch`] and
//!    [`CidImages::insert`] it;
//! 3. [`RawBody::render`] gives the sanitized [`Body`], split at its quoted
//!    history, with text for search.
//!
//! A draft read from Gmail goes through [`RawBody::to_draft`] instead, which
//! splits it at its `gmail_quote`.
//!
//! # Compose
//!
//! [`reply_quote_html`] and [`forward_quote_html`] build Gmail's quote blocks,
//! and [`reply_subject`] and [`forward_subject`] the subjects. To write a
//! draft, join the editor's HTML and the quote, turn its `data:` images back
//! into `cid:` parts with [`data_images_to_cid`], then [`build_mime`].
//!
//! # Headers
//!
//! [`parse_address_list`], [`is_valid_email`], [`decode_encoded_words`], and
//! [`list_unsubscribe`]; [`build_plain`] writes an unsubscribe request.

mod addr;
mod body;
mod charset;
mod cid;
mod encoded_word;
mod headers;
mod outgoing;
mod params;
mod payload;
mod quote;
mod reply;
mod sanitize;
mod text;
mod text_html;
mod unsubscribe;

pub use addr::{is_valid_email, parse_address_list};
pub use body::{Body, DraftBody, RawBody};
pub use cid::{CidImages, data_images_to_cid};
pub use encoded_word::decode_encoded_words;
pub use outgoing::{
    BuildError, InlinePart, OutAttachment, Outgoing, PlainMessage, build_mime, build_plain,
};
pub use payload::{InlineFetch, MessageHeaders, ParsedMessage, Part, PayloadError, parse_message};
pub use quote::{split_gmail_quote, split_quoted_history};
pub use reply::{
    QuoteSource, UtcOffset, forward_quote_html, forward_subject, reply_quote_html, reply_subject,
};
pub use sanitize::sanitize_html;
pub use text::{html_to_text, snippet_text};
pub use text_html::text_to_html;
pub use unsubscribe::{ListUnsubscribe, Mailto, list_unsubscribe};

/// Compiles one of this crate's fixed patterns.
#[expect(
    clippy::expect_used,
    reason = "every pattern is a constant; a bad one fails every test that parses mail"
)]
fn regex(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("a valid built-in pattern")
}

/// `s` with every run of whitespace one space, and the ends trimmed.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}
