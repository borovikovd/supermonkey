use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde::Deserialize;
use supermonkey_lib::domain::GmailAttachmentId;
use supermonkey_lib::mime::{
    CidImages, WireBody, WireHeader, WirePart, data_images_to_cid, decode_encoded_words,
    html_to_text, is_valid_email, list_unsubscribe, parse_address_list, parse_message,
    sanitize_html, split_quoted_history, text_to_html,
};

#[derive(Deserialize)]
struct AddrCase {
    address: String,
    valid: bool,
    why: String,
}

#[test]
fn is_valid_email_matches_shared_fixture_table() {
    let raw = include_str!("fixtures/email-addresses.json");
    let cases: Vec<AddrCase> = serde_json::from_str(raw).unwrap();
    assert!(!cases.is_empty(), "fixture has address cases");
    for c in cases {
        assert_eq!(
            is_valid_email(&c.address),
            c.valid,
            "{}: {}",
            c.address,
            c.why
        );
    }
}

const FORBIDDEN_VECTORS: &[&str] = &[
    "SCRIPT_IN_HEAD",
    "SCRIPT_IN_BODY",
    "SVG_SCRIPT",
    "ONLOAD",
    "ONCLICK",
    "ONERROR",
    "OVER",
    "JS_HREF",
    "JS_HREF_MIXED",
    "data:text/html",
    "FORM_INPUT",
    "FORM_TEXTAREA",
    "IFRAME_FALLBACK",
    "OBJECT_FALLBACK",
    "SVG_TEXT",
    "MATH_TEXT",
    "NOSCRIPT.png",
    "COMMENT_SECRET",
    "LEAKED_TITLE",
    "javascript:",
    "expression(",
    "http-equiv=\"refresh\"",
    "<base",
    "<form",
];

const KEPT_MARKERS: &[&str] = &[
    "KEEP_STYLE_BLOCK",
    "KEEP_ID",
    "KEEP_CLASS",
    "KEEP_DATA_IMAGE",
    "KEEP_REMOTE.png",
    "KEEP_LINK",
    "noopener",
    "KEEP_CELL",
    "KEEP_FONT",
    "KEEP_CSS_TEXT",
    "literal cid:pic@1 stays text",
];

#[test]
fn sanitizer_strips_hostile_vectors_and_keeps_safe_mail_formatting() {
    let hostile = include_str!("fixtures/hostile.html");
    let mut cids = CidImages::new();
    cids.insert("pic@1", "image/png".to_owned(), vec![1, 2, 3, 4]);
    let with_cid = format!("{hostile}<p>literal cid:pic@1 stays text</p><img src=\"cid:pic@1\">");
    let clean = sanitize_html(&with_cid, &cids);
    let lower = clean.to_ascii_lowercase();

    for forbidden in FORBIDDEN_VECTORS {
        assert!(
            !lower.contains(&forbidden.to_ascii_lowercase()),
            "{forbidden:?}"
        );
    }
    for kept in KEPT_MARKERS {
        assert!(clean.contains(kept), "{kept:?}");
    }
    let data_uri = format!("data:image/png;base64,{}", STANDARD.encode([1u8, 2, 3, 4]));
    assert!(clean.contains(&data_uri));

    let text = html_to_text(hostile);
    assert!(!text.contains("LEAKED_TITLE"));
    assert!(!text.contains("KEEP_STYLE_BLOCK"));
    assert!(!text.contains("SCRIPT_IN_BODY"));
    assert!(text.contains("Hello"));
}

fn wire_header(name: &str, value: &str) -> WireHeader {
    WireHeader {
        name: name.to_owned(),
        value: value.to_owned(),
    }
}

fn wire_att(
    id: &str,
    mime: &str,
    filename: &str,
    att_id: &str,
    headers: Vec<WireHeader>,
) -> WirePart {
    WirePart {
        part_id: id.to_owned(),
        mime_type: mime.to_owned(),
        filename: filename.to_owned(),
        headers,
        body: WireBody {
            attachment_id: Some(GmailAttachmentId(att_id.to_owned())),
            size: 100,
            data: None,
        },
        parts: Vec::new(),
    }
}

fn wire_html(id: &str, html: &str, headers: Vec<WireHeader>) -> WirePart {
    WirePart {
        part_id: id.to_owned(),
        mime_type: "text/html".to_owned(),
        headers,
        body: WireBody {
            data: Some(URL_SAFE_NO_PAD.encode(html)),
            ..WireBody::default()
        },
        ..WirePart::default()
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "builds a multi-part MIME fixture with nested message/rfc822 and verifies cid inline selection"
)]
fn payload_handles_unknown_charset_nested_eml_and_cid_inline_rules() {
    let html = r#"<p>See <img src="cid:hero%201@img"> only</p>"#;
    let nested_html = r#"<p>Nested <img src="cid:nested@img"></p>"#;
    let nested = WirePart {
        part_id: "4".to_owned(),
        mime_type: "message/rfc822".to_owned(),
        parts: vec![
            wire_html("4.0", nested_html, Vec::new()),
            wire_att(
                "4.1",
                "image/png",
                "nested-inline.png",
                "att-n-inline",
                vec![wire_header("Content-ID", "<nested@img>")],
            ),
            wire_att(
                "4.2",
                "text/csv",
                "nested-real.csv",
                "att-n-csv",
                vec![wire_header(
                    "Content-Disposition",
                    "attachment; filename=\"nested-real.csv\"",
                )],
            ),
        ],
        ..WirePart::default()
    };
    let root = WirePart {
        mime_type: "multipart/mixed".to_owned(),
        headers: vec![wire_header("Subject", "Attachments")],
        parts: vec![
            wire_html(
                "0",
                html,
                vec![wire_header(
                    "Content-Type",
                    "text/html; charset=x-unknown-fake",
                )],
            ),
            wire_att(
                "1",
                "image/png",
                "hero.png",
                "att-hero",
                vec![wire_header("Content-ID", "<hero 1@img>")],
            ),
            wire_att(
                "2",
                "image/png",
                "unused.png",
                "att-unused",
                vec![
                    wire_header("Content-Disposition", "inline; filename=\"unused.png\""),
                    wire_header("Content-ID", "<unreferenced@img>"),
                ],
            ),
            wire_att(
                "3",
                "application/pdf",
                "doc.pdf",
                "att-pdf",
                vec![wire_header(
                    "Content-Disposition",
                    "inline; filename=\"doc.pdf\"",
                )],
            ),
            nested,
        ],
        ..WirePart::default()
    };

    let parsed = parse_message(&root);
    assert_eq!(parsed.raw_body.html.as_deref(), Some(html));
    let inline_ids: Vec<&str> = parsed
        .inline_fetches()
        .filter_map(|i| i.attachment_id.map(GmailAttachmentId::as_str))
        .collect();
    assert_eq!(inline_ids, vec!["att-hero"]);

    let regular_names: Vec<&str> = parsed
        .parts
        .iter()
        .filter(|p| !p.is_inline)
        .map(|p| p.filename.as_str())
        .collect();
    assert_eq!(
        regular_names,
        vec!["unused.png", "doc.pdf", "nested-real.csv"]
    );
}

#[test]
fn parsers_and_sanitizer_never_panic_on_3000_seeded_inputs() {
    let mut seed: u64 = 0x5EED_CAFE_BABE_1234;
    let mut next_u64 = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let alphabet = b"abcXYZ0123456789 <>\"'():;,=?-_/\\.@%#&!*\r\n\t\x00\xc3\xa9";
    let cids = CidImages::new();

    for _ in 0..3000 {
        let len = usize::try_from(next_u64() % 180).unwrap_or(0);
        let mut raw = Vec::with_capacity(len);
        for _ in 0..len {
            let idx = usize::try_from(next_u64() % u64::try_from(alphabet.len()).unwrap_or(1))
                .unwrap_or(0);
            raw.push(alphabet[idx]);
        }
        let s = String::from_utf8_lossy(&raw);
        std::hint::black_box(parse_address_list(&s));
        std::hint::black_box(decode_encoded_words(&s));
        std::hint::black_box(is_valid_email(&s));
        std::hint::black_box(sanitize_html(&s, &cids));
        std::hint::black_box(html_to_text(&s));
        std::hint::black_box(text_to_html(&s));
        std::hint::black_box(split_quoted_history(&s));
        std::hint::black_box(list_unsubscribe(Some(&s), Some(&s)));
    }
}

#[test]
fn only_a_quote_that_ends_the_message_is_folded_away() {
    let attribution = r#"<div class="moz-cite-prefix">On 02/11/26 09:00, Ana wrote:<br></div>"#;
    let quote = r#"<blockquote type="cite" cite="mid:q1"><p>The earlier question</p><blockquote type="cite"><p>Older still</p></blockquote></blockquote>"#;

    let below = format!("{attribution}{quote}<p>Answer written below the quote</p>");
    assert_eq!(
        split_quoted_history(&below),
        (below.as_str(), None),
        "a reply below the quote stays in view"
    );

    let between =
        format!("<p>First point</p>{attribution}{quote}<p>Second point, after the quote</p>");
    assert_eq!(
        split_quoted_history(&between),
        (between.as_str(), None),
        "a reply around the quote stays in view"
    );

    let above = format!("<p>Answer written above</p>{attribution}{quote}<br>");
    let (shown, folded) = split_quoted_history(&above);
    assert_eq!(
        shown,
        format!("<p>Answer written above</p>{attribution}"),
        "a quote that ends the message is folded"
    );
    assert_eq!(folded, Some(format!("{quote}<br>").as_str()));

    let outlook = r#"<p>Outlook reply</p><div id="appendonsend"></div><hr><div id="divRplyFwdMsg">From: Ana</div><div>The earlier question</div>"#;
    let (shown, folded) = split_quoted_history(outlook);
    assert_eq!(shown, "<p>Outlook reply</p>");
    assert!(folded.is_some_and(|q| q.contains("The earlier question")));
}

#[test]
fn an_email_forwarded_as_an_attachment_is_listed_as_one() {
    let forwarded = WirePart {
        part_id: "1".to_owned(),
        mime_type: "message/rfc822".to_owned(),
        filename: "Trip plan.eml".to_owned(),
        body: WireBody {
            attachment_id: Some(GmailAttachmentId("att-eml".to_owned())),
            size: 4096,
            data: None,
        },
        parts: vec![
            wire_html("1.0", "<p>The forwarded mail</p>", Vec::new()),
            wire_att(
                "1.1",
                "application/pdf",
                "tickets.pdf",
                "att-tickets",
                vec![wire_header(
                    "Content-Disposition",
                    "attachment; filename=\"tickets.pdf\"",
                )],
            ),
        ],
        ..WirePart::default()
    };
    let root = WirePart {
        mime_type: "multipart/mixed".to_owned(),
        parts: vec![
            wire_html("0", "<p>See the attached email.</p>", Vec::new()),
            forwarded,
        ],
        ..WirePart::default()
    };

    let parsed = parse_message(&root);
    assert_eq!(
        parsed.raw_body.html.as_deref(),
        Some("<p>See the attached email.</p>")
    );
    let listed: Vec<(&str, &str, Option<&str>)> = parsed
        .parts
        .iter()
        .filter(|p| !p.is_inline)
        .map(|p| {
            (
                p.filename.as_str(),
                p.mime_type.as_str(),
                p.attachment_id.as_ref().map(GmailAttachmentId::as_str),
            )
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            ("Trip plan.eml", "message/rfc822", Some("att-eml")),
            ("tickets.pdf", "application/pdf", Some("att-tickets")),
        ]
    );
}

#[test]
fn pictures_in_the_body_and_in_the_quote_get_different_content_ids() {
    let one = STANDARD.encode(b"first picture");
    let two = STANDARD.encode(b"second picture");
    let body = format!(r#"<p><img src="data:image/png;base64,{one}"></p>"#);
    let quote = format!(
        r#"<p><img src="data:image/png;base64,{two}"><img src="data:image/png;base64,{one}"></p>"#
    );

    let (body_html, mut parts) = data_images_to_cid(&body, &[]);
    let (quote_html, more) = data_images_to_cid(&quote, &parts);
    parts.extend(more);

    let first = &parts[0].content_id;
    assert!(body_html.contains(&format!("cid:{first}")));
    let ids: Vec<&str> = parts.iter().map(|p| p.content_id.as_str()).collect();
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "each attached picture has its own id: {ids:?}"
    );
    let second = parts
        .iter()
        .find(|p| p.data == b"second picture")
        .map(|p| p.content_id.as_str())
        .unwrap();
    assert_ne!(second, first.as_str());
    assert!(quote_html.contains(&format!("cid:{second}")));
    assert!(
        quote_html.contains(&format!("cid:{first}")),
        "the same picture in the quote reuses the body's part"
    );
}
