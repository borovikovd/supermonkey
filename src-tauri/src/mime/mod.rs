mod html;
mod outgoing;
mod parse;
mod unsubscribe;

pub use html::{
    Body, CidImages, DraftBody, RawBody, data_images_to_cid, html_to_plain, html_to_text,
    sanitize_html, snippet_text, split_gmail_quote, split_quoted_history, text_to_html,
};
pub use outgoing::{
    BuildError, InlinePart, OutAttachment, Outgoing, PlainMessage, QuoteSource, build_mime,
    build_plain, forward_quote_html, forward_subject, reply_quote_html, reply_subject,
};
pub use parse::{
    DRAFT_HEADER, InlineFetch, MessageHeaders, ParsedMessage, Part, PayloadError, WireBody,
    WireHeader, WirePart, decode_base64_lenient, decode_encoded_words, is_valid_email,
    parse_address_list, parse_message,
};
pub use unsubscribe::{ListUnsubscribe, Mailto, list_unsubscribe};

pub(crate) fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(512));
    let mut in_ws = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !out.is_empty() && !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}
