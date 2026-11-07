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

fn header(name: &str, value: &str) -> Value {
    json!({ "name": name, "value": value })
}

fn data(text: &str) -> Value {
    json!({ "size": text.len(), "data": URL_SAFE_NO_PAD.encode(text) })
}

/// A message forwarded as an attachment, as Gmail's `format=full` serves it:
/// the attached message's parts sit under its `message/rfc822` part, which
/// gets `eml_body`.
fn forwarded_as_attachment(eml_body: &Value) -> Value {
    json!({
        "partId": "",
        "mimeType": "multipart/mixed",
        "parts": [
            {
                "partId": "0",
                "mimeType": "text/plain",
                "headers": [header("Content-Type", "text/plain; charset=UTF-8")],
                "body": data("See the message below."),
            },
            {
                "partId": "1",
                "mimeType": "message/rfc822",
                "filename": "Quarterly.eml",
                "headers": [
                    header("Content-Type", "message/rfc822; name=\"Quarterly.eml\""),
                    header("Content-Disposition", "attachment; filename=\"Quarterly.eml\""),
                ],
                "body": eml_body,
                "parts": [{
                    "partId": "1.0",
                    "mimeType": "multipart/mixed",
                    "headers": [header("Subject", "Quarterly numbers")],
                    "parts": [
                        {
                            "partId": "1.0.0",
                            "mimeType": "multipart/alternative",
                            "parts": [
                                {
                                    "partId": "1.0.0.0",
                                    "mimeType": "text/plain",
                                    "body": data("Numbers attached."),
                                },
                                {
                                    "partId": "1.0.0.1",
                                    "mimeType": "text/html",
                                    "body": data("<p>Numbers attached.<img src=\"cid:logo\"></p>"),
                                },
                            ],
                        },
                        {
                            "partId": "1.0.1",
                            "mimeType": "image/png",
                            "filename": "logo.png",
                            "headers": [
                                header("Content-Disposition", "inline; filename=\"logo.png\""),
                                header("Content-ID", "<logo>"),
                            ],
                            "body": { "size": 120, "attachmentId": "ATT-LOGO" },
                        },
                        {
                            "partId": "1.0.2",
                            "mimeType": "application/pdf",
                            "filename": "q3.pdf",
                            "headers": [
                                header("Content-Disposition", "attachment; filename=\"q3.pdf\""),
                            ],
                            "body": { "size": 2048, "attachmentId": "ATT-PDF" },
                        },
                        {
                            "partId": "1.0.3",
                            "mimeType": "text/csv",
                            "filename": "q3.csv",
                            "headers": [
                                header("Content-Disposition", "attachment; filename=\"q3.csv\""),
                            ],
                            "body": data("a,b\n1,2\n"),
                        },
                    ],
                }],
            },
        ],
    })
}

#[test]
fn an_attached_message_lists_the_attachments_inside_it() {
    let parsed = parse_message(&forwarded_as_attachment(&json!({ "size": 0 }))).unwrap();

    // The attached message's body is not this message's body.
    assert_eq!(parsed.body.text.as_deref(), Some("See the message below."));
    assert_eq!(parsed.body.html, None);
    let listed: Vec<(&str, &str, Option<&str>)> = parsed
        .parts
        .iter()
        .map(|p| {
            (
                p.part_id.as_str(),
                p.filename.as_str(),
                p.attachment_id.as_deref(),
            )
        })
        .collect();
    // Not its inline image, nor the .eml, which has no attachment id.
    assert_eq!(
        listed,
        [
            ("1.0.2", "q3.pdf", Some("ATT-PDF")),
            ("1.0.3", "q3.csv", None),
        ]
    );
    assert_eq!(parsed.parts[1].data.as_deref(), Some(&b"a,b\n1,2\n"[..]));
    assert!(parsed.parts.iter().all(|p| !p.is_inline));
    assert_eq!(parsed.inline_images_to_fetch().count(), 0);
}

#[test]
fn an_attached_message_with_an_attachment_id_is_listed_before_its_attachments() {
    let eml = json!({ "size": 4096, "attachmentId": "ATT-EML" });
    let parsed = parse_message(&forwarded_as_attachment(&eml)).unwrap();
    let names: Vec<&str> = parsed.parts.iter().map(|p| p.filename.as_str()).collect();
    assert_eq!(names, ["Quarterly.eml", "q3.pdf", "q3.csv"]);
    assert_eq!(parsed.parts[0].mime_type, "message/rfc822");
}
