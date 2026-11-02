use std::collections::HashMap;

use serde_json::Value;
use ub_mime::{Addr, data_uri, parse_gmail_payload, sanitize_html};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn full_message_with_nested_parts() {
    let msg = parse_gmail_payload(&fixture("full_mixed.json"));

    assert_eq!(
        msg.from,
        Some(Addr::named("Élodie Dupont", "elodie@example.fr"))
    );
    assert_eq!(
        msg.to,
        vec![
            Addr::named("Bob", "bob@example.com"),
            Addr::named("Doe, Jane", "jane@example.com")
        ]
    );
    assert_eq!(
        msg.cc,
        vec![Addr::new("carol@example.com"), Addr::new("dan@example.com")]
    );
    assert_eq!(msg.reply_to, vec![Addr::new("replies@example.fr")]);
    assert!(msg.bcc.is_empty());
    assert_eq!(msg.subject, "Q1 résumé and chart");
    assert_eq!(msg.date_ms, Some(1_736_157_600_000));
    assert_eq!(msg.message_id.as_deref(), Some("CAF=abc123@mail.gmail.com"));
    assert_eq!(msg.in_reply_to.as_deref(), Some("prev-1@example.com"));
    assert_eq!(
        msg.references,
        vec!["root-0@example.com", "prev-1@example.com"]
    );
    assert!(!msg.is_list);
    assert_eq!(msg.ub_draft_id, None);

    assert_eq!(
        msg.text.as_deref(),
        Some("Hi Bob,\n\nThe chart is below.\n\n— Élodie\n")
    );
    let html = msg.html.as_deref().unwrap();
    assert!(html.contains("<img src=\"cid:ii_chart01\""));
    assert!(html.contains("— Élodie"));

    assert_eq!(msg.parts.len(), 3);
    let image = &msg.parts[0];
    assert_eq!(image.part_id, "0.1");
    assert_eq!(image.filename, "chart.png");
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(image.content_id.as_deref(), Some("ii_chart01"));
    assert!(image.inline);
    assert_eq!(image.attachment_id, None);
    let data = image.data.as_ref().unwrap();
    assert_eq!(data.len() as u64, image.size);
    assert!(data.starts_with(b"\x89PNG"));

    let pdf = &msg.parts[1];
    assert_eq!(pdf.part_id, "1");
    assert_eq!(pdf.filename, "Q1 résumé.pdf");
    assert_eq!(pdf.mime_type, "application/pdf");
    assert_eq!(
        pdf.attachment_id.as_deref(),
        Some("ANGjdJ8w-long-attachment-id")
    );
    assert_eq!(pdf.size, 48213);
    assert!(!pdf.inline);
    assert_eq!(pdf.data, None);

    let invite = &msg.parts[2];
    assert_eq!(invite.filename, "invite.ics");
    assert_eq!(invite.mime_type, "text/calendar");
    assert!(!invite.inline);
}

#[test]
fn cid_image_becomes_data_uri_when_sanitized() {
    let msg = parse_gmail_payload(&fixture("full_mixed.json"));
    let cid_data: HashMap<String, String> = msg
        .parts
        .iter()
        .filter_map(|p| {
            Some((
                p.content_id.clone()?,
                data_uri(&p.mime_type, p.data.as_ref()?),
            ))
        })
        .collect();
    let clean = sanitize_html(msg.html.as_deref().unwrap(), &cid_data);
    assert!(!clean.contains("cid:"));
    assert!(
        clean.contains(&format!("src=\"{}\"", cid_data["ii_chart01"])),
        "{clean}"
    );
}

#[test]
fn latin1_single_part_message() {
    let msg = parse_gmail_payload(&fixture("latin1_plain.json"));
    assert_eq!(msg.text.as_deref(), Some("Grüße aus Köln - café"));
    assert_eq!(msg.html, None);
    assert!(msg.parts.is_empty());
    assert_eq!(msg.subject, "Grüße");
    assert_eq!(
        msg.from,
        Some(Addr::named("Müller, Hans", "hans@example.de"))
    );
    assert!(msg.to.is_empty());
    assert_eq!(msg.date_ms, Some(1_736_238_600_000));
    assert_eq!(msg.message_id.as_deref(), Some("latin1@example.de"));
    assert!(msg.is_list);
}

#[test]
fn windows_1252_html_without_padding() {
    let msg = parse_gmail_payload(&fixture("windows1252_html.json"));
    assert_eq!(
        msg.html.as_deref(),
        Some("<p>\u{201c}Smart quotes\u{201d} \u{2013} \u{20ac}5</p>")
    );
    assert_eq!(msg.text, None);
}

#[test]
fn metadata_only_payload() {
    let msg = parse_gmail_payload(&fixture("metadata_only.json"));
    assert_eq!(msg.from, Some(Addr::named("Me", "me@example.com")));
    assert_eq!(msg.bcc, vec![Addr::new("secret@example.com")]);
    assert_eq!(msg.subject, "Re: Plans");
    assert_eq!(msg.ub_draft_id.as_deref(), Some("d-42"));
    assert_eq!(
        msg.in_reply_to.as_deref(),
        Some("CAF=abc123@mail.gmail.com")
    );
    assert_eq!(msg.date_ms, Some(1_736_345_700_000));
    assert_eq!(msg.html, None);
    assert_eq!(msg.text, None);
    assert!(msg.parts.is_empty());
}

#[test]
fn empty_and_odd_payloads_do_not_panic() {
    for payload in [
        serde_json::json!({}),
        serde_json::json!(null),
        serde_json::json!({"mimeType": "multipart/mixed", "parts": "nope"}),
        serde_json::json!({"mimeType": "text/html", "body": {"data": "!!!not base64"}}),
        serde_json::json!({"headers": [{"name": "From"}, {"value": "x"}, 3]}),
    ] {
        let msg = parse_gmail_payload(&payload);
        assert_eq!(msg.html, None);
        assert!(msg.parts.is_empty());
    }
}
