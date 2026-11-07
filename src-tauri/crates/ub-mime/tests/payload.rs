//! Gmail payloads: bodies in their charset, and the parts a message lists
//! (e2e/COVERAGE.md, kept unit tests).

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use ub_mime::{ParsedMessage, parse_message};

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

/// A message as Apple Mail sends it: every attachment `inline`, a PDF and a
/// photo; and a message forwarded as an attachment holding one of each.
fn apple_mail() -> Value {
    let pdf = |id: &str, name: &str| {
        json!({
            "partId": id,
            "mimeType": "application/pdf",
            "filename": name,
            "headers": [
                header("Content-Type", &format!("application/pdf; name=\"{name}\"")),
                header("Content-Disposition", &format!("inline; filename=\"{name}\"")),
            ],
            "body": { "size": 4096, "attachmentId": format!("ATT-{id}") },
        })
    };
    let photo = |id: &str| {
        json!({
            "partId": id,
            "mimeType": "image/jpeg",
            "filename": "photo.jpg",
            "headers": [
                header("Content-Disposition", "inline; filename=\"photo.jpg\""),
                header("Content-ID", &format!("<photo-{id}>")),
            ],
            "body": { "size": 2048, "attachmentId": format!("ATT-{id}") },
        })
    };
    json!({
        "partId": "",
        "mimeType": "multipart/mixed",
        "parts": [
            {
                "partId": "0",
                "mimeType": "text/html",
                "body": data("<p>Scans attached.<img src=\"cid:photo-2\"></p>"),
            },
            pdf("1", "scan.pdf"),
            photo("2"),
            {
                "partId": "3",
                "mimeType": "message/rfc822",
                "filename": "Fwd.eml",
                "headers": [header("Content-Disposition", "attachment; filename=\"Fwd.eml\"")],
                "body": { "size": 0 },
                "parts": [{
                    "partId": "3.0",
                    "mimeType": "multipart/mixed",
                    "parts": [
                        { "partId": "3.0.0", "mimeType": "text/plain", "body": data("Inside.") },
                        pdf("3.0.1", "inner.pdf"),
                        photo("3.0.2"),
                    ],
                }],
            },
        ],
    })
}

/// Each listed part's filename, and whether it is inline.
fn listed(parsed: ParsedMessage) -> Vec<(String, bool)> {
    parsed
        .parts
        .into_iter()
        .map(|p| (p.filename, p.is_inline))
        .collect()
}

fn named(parts: &[(&str, bool)]) -> Vec<(String, bool)> {
    parts.iter().map(|&(n, i)| (n.to_owned(), i)).collect()
}

#[test]
fn a_file_sent_inline_is_an_attachment() {
    // A PDF sent inline is an attachment, here and inside the attached
    // message. The photo the HTML shows is an inline image of this
    // message's body; the attached message's photo is an attachment, since
    // its plain-text body can't show it.
    assert_eq!(
        listed(parse_message(&apple_mail()).unwrap()),
        named(&[
            ("scan.pdf", false),
            ("photo.jpg", true),
            ("inner.pdf", false),
            ("photo.jpg", false),
        ])
    );
}

/// A photo sent `inline`, as Apple Mail sends one, with this Content-ID if any.
fn photo(id: &str, name: &str, content_id: Option<&str>) -> Value {
    let mut headers = vec![header(
        "Content-Disposition",
        &format!("inline; filename=\"{name}\""),
    )];
    if let Some(cid) = content_id {
        headers.push(header("Content-ID", &format!("<{cid}>")));
    }
    json!({
        "partId": id,
        "mimeType": "image/jpeg",
        "filename": name,
        "headers": headers,
        "body": { "size": 2048, "attachmentId": format!("ATT-{id}") },
    })
}

#[test]
fn an_image_is_inline_only_when_the_html_body_shows_it() {
    // Apple Mail with a plain-text body: the photo goes `inline` with no
    // Content-ID.
    let plain_text = json!({
        "partId": "",
        "mimeType": "multipart/mixed",
        "parts": [
            { "partId": "0", "mimeType": "text/plain", "body": data("From my phone.") },
            photo("1", "IMG_0001.jpeg", None),
        ],
    });
    assert_eq!(
        listed(parse_message(&plain_text).unwrap()),
        named(&[("IMG_0001.jpeg", false)])
    );

    // HTML that shows one image, by a percent-encoded reference, but not a
    // second one with a Content-ID, nor a photo with none.
    let html = json!({
        "partId": "",
        "mimeType": "multipart/related",
        "parts": [
            {
                "partId": "0",
                "mimeType": "text/html",
                "body": data("<p>The chart:<img src=\"cid:chart%40mac\"></p>"),
            },
            photo("1", "chart.jpeg", Some("chart@mac")),
            photo("2", "unused.jpeg", Some("unused@mac")),
            photo("3", "IMG_0002.jpeg", None),
        ],
    });
    assert_eq!(
        listed(parse_message(&html).unwrap()),
        named(&[
            ("chart.jpeg", true),
            ("unused.jpeg", false),
            ("IMG_0002.jpeg", false),
        ])
    );
    // Only the image the body shows is fetched for it.
    let parsed = parse_message(&html).unwrap();
    let fetched: Vec<&str> = parsed
        .inline_images_to_fetch()
        .map(|f| f.content_id)
        .collect();
    assert_eq!(fetched, ["chart@mac"]);
}

#[test]
fn an_attached_message_leaves_out_only_the_images_its_html_shows() {
    let forwarded = json!({
        "partId": "",
        "mimeType": "multipart/mixed",
        "parts": [
            { "partId": "0", "mimeType": "text/plain", "body": data("Forwarded.") },
            {
                "partId": "1",
                "mimeType": "message/rfc822",
                "filename": "Fwd.eml",
                "body": { "size": 0 },
                "parts": [{
                    "partId": "1.0",
                    "mimeType": "multipart/related",
                    "parts": [
                        {
                            "partId": "1.0.0",
                            "mimeType": "text/html",
                            "body": data("<p><img src=\"cid:logo@mac\"></p>"),
                        },
                        photo("1.0.1", "logo.jpeg", Some("logo@mac")),
                        photo("1.0.2", "IMG_0003.jpeg", None),
                    ],
                }],
            },
        ],
    });
    assert_eq!(
        listed(parse_message(&forwarded).unwrap()),
        named(&[("IMG_0003.jpeg", false)])
    );
}
