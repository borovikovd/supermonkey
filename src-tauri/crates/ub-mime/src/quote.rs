//! Quote blocks: building Gmail-style reply/forward quotes and finding quoted history.

use std::fmt::Write as _;
use std::sync::LazyLock;

use chrono::{DateTime, FixedOffset, Offset};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::address::Addr;
use crate::html::html_to_text;

/// The message a reply or forward quotes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuoteSource {
    pub from: Addr,
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub date_ms: i64,
    pub subject: String,
    /// The stored (sanitized) HTML of the message. For a plain-text message pass
    /// [`text_to_html`](crate::text_to_html) of its text.
    pub html: String,
}

const GMAIL_QUOTE_START: &str =
    r#"<[a-zA-Z][^>]*\bclass\s*=\s*["']?[^"'>]*\bgmail_quote(?:[\s"'>/])"#;

static GMAIL_QUOTE: LazyLock<Regex> =
    LazyLock::new(|| crate::regex(&format!("(?i){GMAIL_QUOTE_START}")));

static HISTORY_START: LazyLock<Regex> = LazyLock::new(|| {
    let markers = [
        GMAIL_QUOTE_START,
        // Apple Mail, Thunderbird
        r#"<blockquote\b[^>]*\btype\s*=\s*["']?cite\b"#,
        // Thunderbird attribution line, Yahoo, Proton
        r#"<[a-zA-Z][^>]*\bclass\s*=\s*["']?[^"'>]*\b(?:moz-cite-prefix|yahoo_quoted|protonmail_quote)\b"#,
        // Outlook on the web
        r#"<div\b[^>]*\bid\s*=\s*["']?(?:appendonsend|divRplyFwdMsg)\b"#,
        // Outlook desktop: the reply header's top border
        r#"<div\b[^>]*\bstyle\s*=\s*["'][^"']*border-top:\s*solid\s+#(?:E1E1E1|B5C4DF)"#,
    ];
    crate::regex(&format!("(?i){}", markers.join("|")))
});

static TRAILING_BREAKS: LazyLock<Regex> =
    LazyLock::new(|| crate::regex(r"(?i)(?:\s|<br\s*/?>|&nbsp;)+$"));

/// Splits a draft read from Gmail into the part that goes into the editor and the quote block:
/// everything from the first element whose class includes `gmail_quote` onward. Trailing
/// `<br>`s and whitespace are trimmed from the editor part. `before + quote` reproduces the
/// input apart from that trimming, so tags opened before the quote may be closed inside it.
pub fn split_gmail_quote(html: &str) -> (&str, Option<&str>) {
    match GMAIL_QUOTE.find(html) {
        Some(m) => (
            trim_trailing_breaks(&html[..m.start()]),
            Some(&html[m.start()..]),
        ),
        None => (html, None),
    }
}

/// Finds where quoted history starts in a received message, for the `•••` toggle. Markers:
/// Gmail's `gmail_quote`, `<blockquote type="cite">` (Apple Mail, Thunderbird, and plain-text
/// quotes from [`text_to_html`](crate::text_to_html)), Thunderbird/Yahoo/Proton quote classes,
/// and Outlook's reply header. Returns `(visible_html, Some(quoted_html))`, or the whole HTML
/// and `None` when there is no marker, nothing visible above it, or the block is a forward.
pub fn split_quoted_history(html: &str) -> (&str, Option<&str>) {
    let Some(m) = HISTORY_START.find(html) else {
        return (html, None);
    };
    let (visible, quoted) = html.split_at(m.start());
    let quoted_text = html_to_text(quoted);
    let is_forward = quoted_text.starts_with("---------- Forwarded message")
        || quoted_text.starts_with("Begin forwarded message");
    if html_to_text(visible).is_empty() || is_forward {
        return (html, None);
    }
    (trim_trailing_breaks(visible), Some(quoted))
}

fn trim_trailing_breaks(html: &str) -> &str {
    TRAILING_BREAKS
        .find(html)
        .map_or(html, |m| &html[..m.start()])
}

/// Gmail's reply quote block:
///
/// ```html
/// <div class="gmail_quote"><div dir="ltr" class="gmail_attr">On Mon, Jan 6, 2025 at 10:00 AM
/// Alice &lt;<a href="mailto:a@x.com">a@x.com</a>&gt; wrote:<br></div><blockquote
/// class="gmail_quote" style="...">original html</blockquote></div>
/// ```
///
/// `tz_offset_minutes` is the local offset east of UTC (UTC+2 is `120`; note JavaScript's
/// `getTimezoneOffset()` has the opposite sign).
pub fn reply_quote_html(src: &QuoteSource, tz_offset_minutes: i32) -> String {
    format!(
        "<div class=\"gmail_quote\"><div dir=\"ltr\" class=\"gmail_attr\">On {} {} wrote:<br></div>\
         <blockquote class=\"gmail_quote\" style=\"margin:0px 0px 0px 0.8ex;border-left:1px solid rgb(204,204,204);padding-left:1ex\">\
         {}</blockquote></div>",
        format_date(src.date_ms, tz_offset_minutes),
        addr_html(&src.from),
        src.html
    )
}

/// Gmail's forward block: `---------- Forwarded message ---------` and the original From,
/// Date, Subject, To (and Cc when present) lines, then the original HTML, all inside
/// `<div class="gmail_quote">`. See [`reply_quote_html`] for `tz_offset_minutes`.
pub fn forward_quote_html(src: &QuoteSource, tz_offset_minutes: i32) -> String {
    let from = match src.from.name.as_deref().filter(|n| !n.is_empty()) {
        Some(name) => format!(
            "<strong class=\"gmail_sendername\" dir=\"auto\">{}</strong> <span dir=\"auto\">&lt;{}&gt;</span>",
            html_escape::encode_text(name),
            mailto(&src.from.email)
        ),
        None => format!(
            "<span dir=\"auto\">&lt;{}&gt;</span>",
            mailto(&src.from.email)
        ),
    };
    let mut out = format!(
        "<div class=\"gmail_quote\"><div dir=\"ltr\" class=\"gmail_attr\">---------- Forwarded message ---------<br>\
         From: {from}<br>Date: {}<br>Subject: {}<br>To: {}<br>",
        format_date(src.date_ms, tz_offset_minutes),
        html_escape::encode_text(&src.subject),
        addr_list_html(&src.to),
    );
    if !src.cc.is_empty() {
        let _ = write!(out, "Cc: {}<br>", addr_list_html(&src.cc));
    }
    out.push_str("</div><br><br>");
    out.push_str(&src.html);
    out.push_str("</div>");
    out
}

/// `Mon, Jan 6, 2025 at 10:00 AM`, Gmail's quote date format.
fn format_date(date_ms: i64, tz_offset_minutes: i32) -> String {
    let offset = FixedOffset::east_opt(tz_offset_minutes * 60).unwrap_or(chrono::Utc.fix());
    DateTime::from_timestamp_millis(date_ms)
        .unwrap_or_default()
        .with_timezone(&offset)
        .format("%a, %b %-d, %Y at %-I:%M %p")
        .to_string()
}

fn mailto(email: &str) -> String {
    format!(
        "<a href=\"mailto:{}\">{}</a>",
        html_escape::encode_double_quoted_attribute(email),
        html_escape::encode_text(email)
    )
}

/// `Name &lt;<a href="mailto:e">e</a>&gt;`, or just the bracketed link without a name.
fn addr_html(a: &Addr) -> String {
    match a.name.as_deref().filter(|n| !n.is_empty()) {
        Some(name) => format!(
            "{} &lt;{}&gt;",
            html_escape::encode_text(name),
            mailto(&a.email)
        ),
        None => format!("&lt;{}&gt;", mailto(&a.email)),
    }
}

fn addr_list_html(list: &[Addr]) -> String {
    list.iter().map(addr_html).collect::<Vec<_>>().join(", ")
}

/// `Re: subject`, unless the subject already starts with `Re:` (any case).
pub fn reply_subject(s: &str) -> String {
    prefixed(s, "Re:", &["re:"])
}

/// `Fwd: subject`, unless the subject already starts with `Fwd:` or `Fw:` (any case).
pub fn forward_subject(s: &str) -> String {
    prefixed(s, "Fwd:", &["fwd:", "fw:"])
}

fn prefixed(s: &str, prefix: &str, existing: &[&str]) -> String {
    let s = s.trim();
    let lower = s.to_lowercase();
    if existing.iter().any(|p| lower.starts_with(p)) {
        s.to_string()
    } else if s.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix} {s}")
    }
}
