//! Finding quoted history in HTML: Gmail's `gmail_quote` for drafts, and the
//! common clients' markers for the `•••` toggle (SPEC Reading, Reply composer).

use std::sync::LazyLock;

use regex::Regex;

use crate::text::html_to_text;

/// The start tag of an element whose class includes `gmail_quote`.
const GMAIL_QUOTE_START: &str =
    r#"<[a-zA-Z][^>]*\bclass\s*=\s*["']?[^"'>]*\bgmail_quote(?:[\s"'>/])"#;

static GMAIL_QUOTE: LazyLock<Regex> =
    LazyLock::new(|| crate::regex(&format!("(?i){GMAIL_QUOTE_START}")));

static HISTORY_START: LazyLock<Regex> = LazyLock::new(|| {
    let markers = [
        GMAIL_QUOTE_START,
        // Apple Mail, Thunderbird, and plain-text quotes from `text_to_html`.
        r#"<blockquote\b[^>]*\btype\s*=\s*["']?cite\b"#,
        // Thunderbird's attribution line, Yahoo, Proton.
        r#"<[a-zA-Z][^>]*\bclass\s*=\s*["']?[^"'>]*\b(?:moz-cite-prefix|yahoo_quoted|protonmail_quote)\b"#,
        // Outlook on the web.
        r#"<div\b[^>]*\bid\s*=\s*["']?(?:appendonsend|divRplyFwdMsg)\b"#,
        // Outlook desktop: the top border of its reply header.
        r#"<div\b[^>]*\bstyle\s*=\s*["'][^"']*border-top:\s*solid\s+#(?:E1E1E1|B5C4DF)"#,
    ];
    crate::regex(&format!("(?i){}", markers.join("|")))
});

static TRAILING_BREAKS: LazyLock<Regex> =
    LazyLock::new(|| crate::regex(r"(?i)(?:\s|<br\s*/?>|&nbsp;)+$"));

/// Splits a draft read from Gmail into the editor's part and the quote block:
/// everything from the first element whose class includes `gmail_quote` on.
/// Trailing `<br>`s and whitespace are trimmed from the editor's part; apart
/// from that, the two halves are the input, so a tag opened before the quote
/// may close inside it.
pub fn split_gmail_quote(html: &str) -> (&str, Option<&str>) {
    match GMAIL_QUOTE.find(html) {
        Some(m) => {
            let (above, quote) = html.split_at(m.start());
            (trim_trailing_breaks(above), Some(quote))
        }
        None => (html, None),
    }
}

/// Splits a received message's HTML at its quoted history, for the `•••`
/// toggle. Returns the whole HTML and `None` when there is no marker, nothing
/// visible above it, or the quoted block is a forward (which is the content).
pub fn split_quoted_history(html: &str) -> (&str, Option<&str>) {
    let Some(m) = HISTORY_START.find(html) else {
        return (html, None);
    };
    let (visible, quoted) = html.split_at(m.start());
    let quoted_text = html_to_text(quoted);
    let is_forward = quoted_text.starts_with("---------- Forwarded message")
        || quoted_text.starts_with("Begin forwarded message");
    if is_forward || html_to_text(visible).is_empty() {
        return (html, None);
    }
    (trim_trailing_breaks(visible), Some(quoted))
}

fn trim_trailing_breaks(html: &str) -> &str {
    TRAILING_BREAKS
        .find(html)
        .map_or(html, |m| html.get(..m.start()).unwrap_or(html))
}
