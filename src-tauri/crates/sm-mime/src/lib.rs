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

#[expect(
    clippy::expect_used,
    reason = "every pattern is a constant; a bad one fails every test that parses mail"
)]
fn regex(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("a valid built-in pattern")
}

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
