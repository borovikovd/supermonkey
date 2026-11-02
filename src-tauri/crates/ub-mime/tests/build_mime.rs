#![expect(clippy::unwrap_used, reason = "Tests fail loudly on purpose.")]
use mail_parser::{MessageParser, MessagePart};
use ub_mime::{
    Addr, InlinePart, OutAttachment, Outgoing, QuoteSource, build_mime, data_images_to_cid,
    data_uri, reply_quote_html, split_gmail_quote,
};

fn png() -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend((0..=255u8).cycle().take(3000));
    bytes
}

fn outgoing() -> Outgoing {
    Outgoing {
        from: Addr::named("Élodie Dupont", "elodie@example.fr"),
        to: vec![
            Addr::named("Bob", "bob@example.com"),
            Addr::named("Doe, Jane", "jane@example.com"),
        ],
        cc: vec![Addr::new("carol@example.com")],
        bcc: vec![Addr::new("secret@example.com")],
        subject: "Re: Q1 résumé — 日本語".into(),
        html: "<p>Thanks!</p>".into(),
        in_reply_to: Some("<CAF=abc123@mail.gmail.com>".into()),
        references: vec![
            "root-0@example.com".into(),
            "<CAF=abc123@mail.gmail.com>".into(),
        ],
        ub_draft_id: "d-42".into(),
        attachments: vec![],
        inline: vec![],
        date_ms: 1_736_157_600_000,
        message_id_domain: "ubermensch.local".into(),
    }
}

/// `multipart/mixed[multipart/related[...], application/pdf]`
fn tree(part: &MessagePart<'_>) -> String {
    let ct = part.content_type().unwrap();
    let name = format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or_default());
    if part.is_multipart() {
        let children: Vec<String> = part.children().map(|c| tree(&c)).collect();
        format!("{name}[{}]", children.join(", "))
    } else {
        name
    }
}

#[test]
fn headers_round_trip() {
    let raw = build_mime(&outgoing());
    let msg = MessageParser::new().parse(&raw).unwrap();

    assert_eq!(msg.subject(), Some("Re: Q1 résumé — 日本語"));
    let text = String::from_utf8_lossy(&raw);
    assert!(
        text.contains("Subject: =?utf-8?"),
        "subject is not encoded:\n{text}"
    );
    let from = msg.from().unwrap().first().unwrap();
    assert_eq!(
        (from.name(), from.address()),
        (Some("Élodie Dupont"), Some("elodie@example.fr"))
    );
    let to: Vec<_> = msg
        .to()
        .unwrap()
        .mailboxes()
        .map(|m| (m.name(), m.address()))
        .collect();
    assert_eq!(
        to,
        vec![
            (Some("Bob"), Some("bob@example.com")),
            (Some("Doe, Jane"), Some("jane@example.com"))
        ]
    );
    assert_eq!(
        msg.cc().unwrap().first().unwrap().address(),
        Some("carol@example.com")
    );
    assert_eq!(
        msg.bcc().unwrap().first().unwrap().address(),
        Some("secret@example.com")
    );
    assert_eq!(
        msg.message_id(),
        Some("ub.d-42.1736157600000@ubermensch.local")
    );
    assert_eq!(
        msg.in_reply_to().unwrap().first(),
        Some("CAF=abc123@mail.gmail.com")
    );
    let refs = msg.references().unwrap();
    assert_eq!(
        (refs.get(0), refs.get(1)),
        (
            Some("root-0@example.com"),
            Some("CAF=abc123@mail.gmail.com")
        )
    );
    assert_eq!(msg.date().unwrap().to_timestamp(), 1_736_157_600);
    assert!(text.contains("\r\nX-Ubermensch-Draft: d-42\r\n"));
    assert!(text.contains("MIME-Version: 1.0"));

    // Our own parser reads it back the same way.
    let headers = [
        (
            "Subject",
            "=?utf-8?B?UmU6IFExIHLDqXN1bcOpIOKAlCDml6XmnKzoqp4=?=",
        ),
        (
            "From",
            text.lines()
                .find(|l| l.starts_with("From: "))
                .unwrap()
                .trim_start_matches("From: "),
        ),
    ];
    let to_header = text
        .split("\r\nTo: ")
        .nth(1)
        .unwrap()
        .split("\r\nCc:")
        .next()
        .unwrap();
    assert_eq!(
        ub_mime::parse_address_list(&to_header.replace("\r\n", "")),
        outgoing().to
    );
    assert_eq!(
        ub_mime::decode_encoded_words(headers[0].1),
        "Re: Q1 résumé — 日本語"
    );
    assert_eq!(
        ub_mime::parse_address_list(headers[1].1),
        vec![outgoing().from]
    );
}

#[test]
fn alternative_is_top_without_parts() {
    let mut o = outgoing();
    o.html = "<p>Hello <b>Bob</b>,</p><p>see <a href=\"https://x.com\">this</a></p>".into();
    let raw = build_mime(&o);
    let msg = MessageParser::new().parse(&raw).unwrap();
    assert_eq!(
        tree(&msg.root_part()),
        "multipart/alternative[text/plain, text/html]"
    );
    assert_eq!(msg.body_html(0).as_deref(), Some(o.html.as_str()));
    // MIME text parts use CRLF line endings.
    assert_eq!(
        msg.body_text(0).as_deref(),
        Some("Hello Bob,\r\n\r\nsee this <https://x.com>\r\n")
    );
}

#[test]
fn related_is_top_with_inline_only() {
    let mut o = outgoing();
    o.html = "<p>chart:</p><img src=\"cid:ii_chart01\">".into();
    o.inline = vec![InlinePart {
        content_id: "ii_chart01".into(),
        mime_type: "image/png".into(),
        filename: Some("chart.png".into()),
        data: png(),
    }];
    let raw = build_mime(&o);
    let msg = MessageParser::new().parse(&raw).unwrap();
    assert_eq!(
        tree(&msg.root_part()),
        "multipart/related[multipart/alternative[text/plain, text/html], image/png]"
    );
    let image = msg.parts().find(|p| p.content_id().is_some()).unwrap();
    assert_eq!(image.content_id(), Some("ii_chart01"));
    assert!(image.is_inline());
    assert_eq!(image.attachment_name(), Some("chart.png"));
    assert_eq!(image.decoded().as_ref(), png().as_slice());
}

#[test]
fn mixed_is_top_with_attachments() {
    let mut o = outgoing();
    o.html = "<p>files</p><img src=\"cid:ii_chart01\">".into();
    o.inline = vec![InlinePart {
        content_id: "<ii_chart01>".into(),
        mime_type: "image/png".into(),
        filename: None,
        data: png(),
    }];
    o.attachments = vec![
        OutAttachment {
            filename: "Q1 résumé, final.pdf".into(),
            mime_type: "application/pdf".into(),
            data: b"%PDF-1.7 fake".to_vec(),
        },
        OutAttachment {
            filename: "notes.txt".into(),
            mime_type: "text/plain".into(),
            data: NOTES.to_vec(),
        },
    ];
    let raw = build_mime(&o);
    let msg = MessageParser::new().parse(&raw).unwrap();
    assert_eq!(
        tree(&msg.root_part()),
        "multipart/mixed[multipart/related[multipart/alternative[text/plain, text/html], image/png], \
         application/pdf, text/plain]"
    );
    let names: Vec<_> = msg
        .attachments()
        .filter_map(|a| a.attachment_name())
        .collect();
    assert_eq!(names, vec!["Q1 résumé, final.pdf", "notes.txt"]);
    let pdf = msg
        .attachments()
        .find(|a| a.attachment_name() == Some("Q1 résumé, final.pdf"))
        .unwrap();
    assert_eq!(pdf.decoded().as_ref(), b"%PDF-1.7 fake");
    let notes = msg
        .attachments()
        .find(|a| a.attachment_name() == Some("notes.txt"))
        .unwrap();
    assert_eq!(
        notes.decoded().as_ref(),
        NOTES,
        "attachment bytes are kept exactly"
    );
    let image = msg.parts().find(|p| p.content_id().is_some()).unwrap();
    assert_eq!(image.content_id(), Some("ii_chart01"));
}

const NOTES: &[u8] = b"line1\nline2\r\nline3\rend \n";

#[test]
fn reply_with_quoted_inline_image_round_trip() {
    let image = InlinePart {
        content_id: "ii_chart01".into(),
        mime_type: "image/png".into(),
        filename: Some("chart.png".into()),
        data: png(),
    };
    let other = InlinePart {
        content_id: "unused".into(),
        mime_type: "image/gif".into(),
        filename: None,
        data: b"GIF89a".to_vec(),
    };
    // The stored message has its cid image as a data: URI (from sanitize_html).
    let stored = format!(
        "<div>Chart: <img src=\"{}\" alt=\"c\"></div>",
        data_uri("image/png", &image.data)
    );
    let quote = reply_quote_html(
        &QuoteSource {
            from: Addr::named("Élodie", "elodie@example.fr"),
            to: vec![],
            cc: vec![],
            date_ms: 1_736_157_600_000,
            subject: "Q1".into(),
            html: stored,
        },
        0,
    );
    let foreign = "data:image/gif;base64,R0lGODlhAQABAAAAACw=";
    let full = format!("<p>Looks good <img src=\"{foreign}\"></p>{quote}");

    let (html, used) = data_images_to_cid(&full, &[other, image.clone()]);
    assert_eq!(used, vec![image.clone()]);
    assert!(html.contains("<img src=\"cid:ii_chart01\" alt=\"c\">"));
    assert!(html.contains(foreign), "unknown data: images stay");
    assert!(!html.contains("data:image/png"));

    let mut o = outgoing();
    o.html = html.clone();
    o.inline = used;
    let raw = build_mime(&o);
    assert!(
        raw.split(|b| *b == b'\n').all(|line| line.len() <= 998),
        "RFC 5322 line limit"
    );
    let msg = MessageParser::new().parse(&raw).unwrap();
    let body = msg.body_html(0).unwrap();
    assert_eq!(body, html);
    let (editor, quote_back) = split_gmail_quote(&body);
    assert_eq!(editor, format!("<p>Looks good <img src=\"{foreign}\"></p>"));
    assert!(quote_back.unwrap().contains("cid:ii_chart01"));
    let plain = msg.body_text(0).unwrap().replace("\r\n", "\n");
    assert!(plain.contains("Looks good\n"), "{plain}");
    assert!(plain.contains("> Chart:"), "{plain}");
}

#[test]
fn data_images_to_cid_dedupes_and_ignores_unknown() {
    let a = InlinePart {
        content_id: "a".into(),
        mime_type: "image/png".into(),
        filename: None,
        data: vec![1, 2, 3],
    };
    let uri = data_uri("image/png", &a.data);
    let html =
        format!("<img src=\"{uri}\"><img src='{uri}'><img src=\"data:image/png;base64,BAUG\">");
    let (out, used) = data_images_to_cid(&html, std::slice::from_ref(&a));
    assert_eq!(
        out,
        "<img src=\"cid:a\"><img src='cid:a'><img src=\"data:image/png;base64,BAUG\">"
    );
    assert_eq!(used, vec![a]);
    assert_eq!(
        data_images_to_cid("<p>none</p>", &[]),
        ("<p>none</p>".to_string(), vec![])
    );
}
