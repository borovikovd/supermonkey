//! Random input built from the characters these parsers care about must never panic.

use std::collections::HashMap;

use ub_mime::{
    decode_encoded_words, html_to_text, parse_address_list, sanitize_html, split_gmail_quote,
    split_quoted_history, text_to_html,
};

const PIECES: &[&str] = &[
    "<",
    ">",
    "\"",
    "'",
    "\\",
    "(",
    ")",
    ",",
    ";",
    ":",
    "@",
    " ",
    "\r\n",
    "\t",
    "=?",
    "?=",
    "?B?",
    "?Q?",
    "UTF-8",
    "=C3",
    "_",
    "é",
    "日",
    "a",
    "x.com",
    "mailto:",
    "<br>",
    "</a>",
    "<a href=\"",
    "https://",
    "<pre>",
    "</pre>",
    "<blockquote type=\"cite\">",
    "class=\"gmail_quote\"",
    "&nbsp;",
    "&#",
    "<!--",
    "-->",
    "cid:",
    "> ",
    "wrote:",
    "\n",
    "%",
    "%C3%A9",
];

fn random_inputs() -> impl Iterator<Item = String> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    (0..3000).map(move |_| {
        let len = (next() % 40) as usize;
        (0..len)
            .map(|_| PIECES[(next() % PIECES.len() as u64) as usize])
            .collect()
    })
}

#[test]
fn parsers_never_panic() {
    let cids = HashMap::from([("a".to_string(), "data:image/png;base64,AA==".to_string())]);
    for input in random_inputs() {
        let _ = parse_address_list(&input);
        let _ = decode_encoded_words(&input);
        let _ = html_to_text(&input);
        let _ = text_to_html(&input);
        let _ = split_gmail_quote(&input);
        let _ = split_quoted_history(&input);
        let _ = sanitize_html(&input, &cids);
    }
}
