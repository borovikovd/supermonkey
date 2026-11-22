use std::sync::LazyLock;

use regex::Regex;

use crate::text::html_to_text;

const GMAIL_QUOTE_START: &str =
    r#"<[a-zA-Z][^>]*\bclass\s*=\s*["']?[^"'>]*\bgmail_quote(?:[\s"'>/])"#;

static GMAIL_QUOTE: LazyLock<Regex> =
    LazyLock::new(|| crate::regex(&format!("(?i){GMAIL_QUOTE_START}")));

static HISTORY_START: LazyLock<Regex> = LazyLock::new(|| {
    let markers = [
        GMAIL_QUOTE_START,
        r#"<blockquote\b[^>]*\btype\s*=\s*["']?cite\b"#,
        r#"<[a-zA-Z][^>]*\bclass\s*=\s*["']?[^"'>]*\b(?:moz-cite-prefix|yahoo_quoted|protonmail_quote)\b"#,
        r#"<div\b[^>]*\bid\s*=\s*["']?(?:appendonsend|divRplyFwdMsg)\b"#,
        r#"<div\b[^>]*\bstyle\s*=\s*["'][^"']*border-top:\s*solid\s+#(?:E1E1E1|B5C4DF)"#,
    ];
    crate::regex(&format!("(?i){}", markers.join("|")))
});

static TRAILING_BREAKS: LazyLock<Regex> =
    LazyLock::new(|| crate::regex(r"(?i)(?:\s|<br\s*/?>|&nbsp;)+$"));

pub fn split_gmail_quote(html: &str) -> (&str, Option<&str>) {
    match GMAIL_QUOTE.find(html) {
        Some(m) => {
            let (above, quote) = html.split_at(m.start());
            (trim_trailing_breaks(above), Some(quote))
        }
        None => (html, None),
    }
}

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
