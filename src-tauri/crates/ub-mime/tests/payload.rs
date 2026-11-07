//! Gmail payloads: bodies in their charset, and the parts a message lists
//! (e2e/COVERAGE.md, kept unit tests).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use ub_mime::parse_message;

fn plain(charset: &str, bytes: &[u8]) -> Value {
    json!({
        "partId": "",
        "mimeType": "text/plain",
        "headers": [{ "name": "Content-Type", "value": format!("text/plain; charset={charset}") }],
        "body": { "size": bytes.len(), "data": URL_SAFE_NO_PAD.encode(bytes) },
    })
}

#[test]
fn a_charset_with_no_decoder_reads_as_undeclared_text() {
    // encoding_rs maps these labels to its "replacement" encoding, which turns
    // a whole body into one U+FFFD.
    for charset in [
        "iso-2022-kr",
        "csiso2022kr",
        "hz-gb-2312",
        "iso-2022-cn",
        "iso-2022-cn-ext",
        "replacement",
    ] {
        let ascii = parse_message(&plain(charset, b"Hello, world")).unwrap();
        assert_eq!(
            ascii.body.text.as_deref(),
            Some("Hello, world"),
            "{charset}"
        );
        let latin1 = parse_message(&plain(charset, b"caf\xe9")).unwrap();
        assert_eq!(latin1.body.text.as_deref(), Some("caf\u{e9}"), "{charset}");
    }
}
