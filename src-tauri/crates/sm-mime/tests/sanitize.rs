//! The sanitizer against hostile HTML (e2e/COVERAGE.md, kept unit tests).

use sm_mime::{CidImages, html_to_text, sanitize_html};

/// Kept byte for byte: the expectations below quote it.
const HOSTILE: &str = include_str!("fixtures/hostile.html");

#[test]
fn hostile_html_is_defused() {
    let clean = sanitize_html(HOSTILE, &CidImages::new());
    let lower = clean.to_lowercase();

    for gone in [
        "<script",
        "script_in_head",
        "script_in_body",
        "onload",
        "onclick",
        "onerror",
        "onmouseover",
        "javascript:",
        "<form",
        "<input",
        "<button",
        "<textarea",
        "form_input",
        "form_textarea",
        "formaction",
        "<iframe",
        "iframe_fallback",
        "<object",
        "object_fallback",
        "<embed",
        "<svg",
        "svg_script",
        "svg_text",
        "<math",
        "math_text",
        "<meta",
        "refresh",
        "<base",
        "<link",
        "leaked_title",
        "<noscript",
        "noscript.png",
        "data:text/html",
        "comment_secret",
        "evil.example",
    ] {
        assert!(!lower.contains(gone), "{gone:?} survived:\n{clean}");
    }

    for kept in [
        "<style",
        ".KEEP_STYLE_BLOCK { color: #333;",
        "table.layout td { padding: 4px; }",
        "id=\"KEEP_ID\"",
        "class=\"KEEP_CLASS\"",
        "style=\"KEEP_STYLE: 1px; color: red\"",
        "data:image/png;base64,iVBORw0KGgo",
        "alt=\"KEEP_DATA_IMAGE\"",
        "src=\"https://cdn.example.com/KEEP_REMOTE.png\" width=\"600\" height=\"100\"",
        "href=\"https://example.com/KEEP_LINK\"",
        "rel=\"noopener noreferrer\"",
        "<table class=\"layout\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" bgcolor=\"#eeeeee\" align=\"center\" style=\"KEEP_TABLE_STYLE: 0\">",
        "<td valign=\"top\" align=\"left\" width=\"50%\">KEEP_CELL</td>",
        "style=\"background-image:url(https://cdn.example.com/bg.png)\"",
        "<font face=\"Georgia\" color=\"#123456\" size=\"2\">KEEP_FONT</font>",
        "DATA_HTML_LINK",
        "<div style=\"color: red; background: url()\">KEEP_CSS_TEXT</div>",
        ".x { background: url() } .y { background: url() }",
    ] {
        assert!(clean.contains(kept), "{kept:?} missing:\n{clean}");
    }
}

#[test]
fn cid_references_are_rewritten_before_sanitizing() {
    let gif = [0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00];
    let uri = "data:image/gif;base64,R0lGODlhAQA=";
    let mut images = CidImages::new();
    images.insert("ii_abc", "image/gif", &gif);
    images.insert("<logo@corp.example>", "image/gif", &gif);
    let html = r#"<img src="cid:ii_abc"><img src='CID:logo%40corp.example'><img src="cid:unknown"><table><tr><td background="cid:ii_abc">x</td></tr></table>"#;
    let clean = sanitize_html(html, &images);
    assert_eq!(clean.matches(uri).count(), 3, "{clean}");
    // Unknown cid: is not an allowed scheme, so the attribute goes.
    assert!(!clean.contains("cid:unknown"));
}

#[test]
fn data_urls_only_for_images() {
    let html = r#"<img src="data:text/html,<b>x</b>"><a href="data:image/png;base64,AAAA">a</a><img src="data:image/gif;base64,R0lG">"#;
    let clean = sanitize_html(html, &CidImages::new());
    assert_eq!(
        clean,
        r#"<img><a rel="noopener noreferrer">a</a><img src="data:image/gif;base64,R0lG">"#
    );
}

#[test]
fn css_escapes_and_entities_do_not_hide_a_script_url() {
    let html = r#"<div style="background:url('\6a avascript:alert(1)')">a</div><div style="background:url(&quot;java\script:x&quot;)">b</div><div style="background:url(https://cdn.example.com/ok.png)">c</div>"#;
    let clean = sanitize_html(html, &CidImages::new());
    assert_eq!(clean.matches("url()").count(), 2, "{clean}");
    assert!(
        clean.contains("url(https://cdn.example.com/ok.png)"),
        "{clean}"
    );
}

#[test]
fn text_leaves_out_a_style_block_in_the_body() {
    assert_eq!(
        html_to_text("<div><style>p { color: red }</style>Hi there</div>"),
        "Hi there"
    );
}

#[test]
fn text_keeps_words_after_a_lone_quote_in_an_unquoted_attribute() {
    assert_eq!(
        html_to_text("<p title=it's>Hello world</p> <p>don't stop</p>"),
        "Hello world don't stop"
    );
    assert_eq!(
        html_to_text(r#"<a href=x title=a"b>one</a> <i class='c d'>two</i> <b id="e>f">three</b>"#),
        "one two three"
    );
}

#[test]
fn text_of_hostile_html_has_no_hidden_content() {
    let text = html_to_text(HOSTILE);
    assert!(text.starts_with("Hello"), "{text}");
    assert!(text.contains("KEEP_CELL KEEP_FONT"));
    for hidden in [
        "DOCTYPE",
        "KEEP_STYLE_BLOCK",
        "SCRIPT_IN",
        "LEAKED_TITLE",
        "COMMENT_SECRET",
        "  ",
    ] {
        assert!(!text.contains(hidden), "{hidden:?} in {text}");
    }
}
