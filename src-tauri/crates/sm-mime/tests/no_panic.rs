//! Random input built from the characters these parsers care about must never
//! panic (e2e/COVERAGE.md, kept unit tests).

use serde_json::json;
use sm_mime::{
    CidImages, RawBody, decode_encoded_words, html_to_text, list_unsubscribe, parse_address_list,
    parse_message, sanitize_html, split_gmail_quote, split_quoted_history, text_to_html,
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
        let len = usize::try_from(next() % 40).unwrap_or(0);
        (0..len)
            .map(|_| PIECES[usize::try_from(next()).unwrap_or(0) % PIECES.len()])
            .collect()
    })
}

#[test]
fn parsers_never_panic() {
    let mut images = CidImages::new();
    images.insert("a", "image/png", &[0]);
    for input in random_inputs() {
        // Only that each returns: what it returns is not checked here.
        _ = parse_address_list(&input);
        _ = decode_encoded_words(&input);
        _ = html_to_text(&input);
        _ = text_to_html(&input);
        _ = split_gmail_quote(&input);
        _ = split_quoted_history(&input);
        _ = sanitize_html(&input, &images);
        _ = list_unsubscribe(&input, Some(&input));

        let header = |name: &str| json!({ "name": name, "value": input });
        let payload = json!({
            "mimeType": "multipart/mixed",
            "headers": [header("From"), header("To"), header("Subject"), header("Date"),
                        header("Message-ID"), header("References"), header("X-Supermonkey-Draft")],
            "parts": [
                { "partId": "0", "mimeType": "text/html",
                  "headers": [header("Content-Type")], "body": { "data": input } },
                { "partId": "1", "mimeType": "image/png", "filename": input,
                  "headers": [header("Content-Disposition"), header("Content-ID")],
                  "body": { "attachmentId": input, "size": 1 } },
            ],
        });
        let parsed = parse_message(&payload).unwrap();
        _ = parsed.inline_images_to_fetch().count();
        _ = parsed.body.clone().render(&parsed.cid_images());
        _ = parsed.body.to_draft();
        _ = RawBody {
            html: None,
            text: Some(input),
        }
        .render(&images);
    }
}
