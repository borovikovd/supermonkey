//! Quote blocks: building Gmail-style reply/forward quotes and finding quoted history.

use std::sync::LazyLock;

use chrono::{DateTime, FixedOffset};
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
    LazyLock::new(|| Regex::new(&format!("(?i){GMAIL_QUOTE_START}")).unwrap());

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
    Regex::new(&format!("(?i){}", markers.join("|"))).unwrap()
});

static TRAILING_BREAKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:\s|<br\s*/?>|&nbsp;)+$").unwrap());

/// Splits a draft read from Gmail into the part that goes into the editor and the quote block:
/// everything from the first element whose class includes `gmail_quote` onward. Trailing
/// `<br>`s and whitespace are trimmed from the editor part. `before + quote` reproduces the
/// input apart from that trimming, so tags opened before the quote may be closed inside it.
pub fn split_gmail_quote(html: &str) -> (String, Option<String>) {
    match GMAIL_QUOTE.find(html) {
        Some(m) => (
            trim_trailing_breaks(&html[..m.start()]),
            Some(html[m.start()..].to_string()),
        ),
        None => (html.to_string(), None),
    }
}

/// Finds where quoted history starts in a received message, for the `•••` toggle. Markers:
/// Gmail's `gmail_quote`, `<blockquote type="cite">` (Apple Mail, Thunderbird, and plain-text
/// quotes from [`text_to_html`](crate::text_to_html)), Thunderbird/Yahoo/Proton quote classes,
/// and Outlook's reply header. Returns `(visible_html, Some(quoted_html))`, or the whole HTML
/// and `None` when there is no marker, nothing visible above it, or the block is a forward.
pub fn split_quoted_history(html: &str) -> (String, Option<String>) {
    let Some(m) = HISTORY_START.find(html) else {
        return (html.to_string(), None);
    };
    let (visible, quoted) = html.split_at(m.start());
    let quoted_text = html_to_text(quoted);
    let is_forward = quoted_text.starts_with("---------- Forwarded message")
        || quoted_text.starts_with("Begin forwarded message");
    if html_to_text(visible).is_empty() || is_forward {
        return (html.to_string(), None);
    }
    (trim_trailing_breaks(visible), Some(quoted.to_string()))
}

fn trim_trailing_breaks(html: &str) -> String {
    TRAILING_BREAKS.replace(html, "").into_owned()
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
        out.push_str(&format!("Cc: {}<br>", addr_list_html(&src.cc)));
    }
    out.push_str("</div><br><br>");
    out.push_str(&src.html);
    out.push_str("</div>");
    out
}

/// `Mon, Jan 6, 2025 at 10:00 AM`, Gmail's quote date format.
fn format_date(date_ms: i64, tz_offset_minutes: i32) -> String {
    let offset =
        FixedOffset::east_opt(tz_offset_minutes * 60).unwrap_or(FixedOffset::east_opt(0).unwrap());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> QuoteSource {
        QuoteSource {
            from: Addr::named("Alice <Smith>", "alice@example.com"),
            to: vec![
                Addr::named("Bob", "bob@example.com"),
                Addr::new("carol@example.com"),
            ],
            cc: vec![Addr::new("dan@example.com")],
            // 2025-01-06 10:00 UTC, a Monday
            date_ms: 1_736_157_600_000,
            subject: "Plans & ideas".into(),
            html: "<div dir=\"ltr\">Original</div>".into(),
        }
    }

    #[test]
    fn reply_quote_matches_gmail() {
        assert_eq!(
            reply_quote_html(&source(), 0),
            "<div class=\"gmail_quote\"><div dir=\"ltr\" class=\"gmail_attr\">On Mon, Jan 6, 2025 at 10:00 AM \
             Alice &lt;Smith&gt; &lt;<a href=\"mailto:alice@example.com\">alice@example.com</a>&gt; wrote:<br></div>\
             <blockquote class=\"gmail_quote\" style=\"margin:0px 0px 0px 0.8ex;border-left:1px solid rgb(204,204,204);padding-left:1ex\">\
             <div dir=\"ltr\">Original</div></blockquote></div>"
        );
        // UTC-8 is 2:00 AM the same day; UTC+14 is midnight the next day.
        assert!(reply_quote_html(&source(), -8 * 60).contains("On Mon, Jan 6, 2025 at 2:00 AM "));
        assert!(reply_quote_html(&source(), 14 * 60).contains("On Tue, Jan 7, 2025 at 12:00 AM "));
    }

    #[test]
    fn forward_quote_matches_gmail() {
        let html = forward_quote_html(&source(), 60);
        assert_eq!(
            html,
            "<div class=\"gmail_quote\"><div dir=\"ltr\" class=\"gmail_attr\">---------- Forwarded message ---------<br>\
             From: <strong class=\"gmail_sendername\" dir=\"auto\">Alice &lt;Smith&gt;</strong> \
             <span dir=\"auto\">&lt;<a href=\"mailto:alice@example.com\">alice@example.com</a>&gt;</span><br>\
             Date: Mon, Jan 6, 2025 at 11:00 AM<br>Subject: Plans &amp; ideas<br>\
             To: Bob &lt;<a href=\"mailto:bob@example.com\">bob@example.com</a>&gt;, \
             &lt;<a href=\"mailto:carol@example.com\">carol@example.com</a>&gt;<br>\
             Cc: &lt;<a href=\"mailto:dan@example.com\">dan@example.com</a>&gt;<br></div><br><br>\
             <div dir=\"ltr\">Original</div></div>"
        );
        let mut no_cc = source();
        no_cc.cc.clear();
        assert!(!forward_quote_html(&no_cc, 0).contains("Cc:"));
    }

    #[test]
    fn subjects() {
        assert_eq!(reply_subject("Hello"), "Re: Hello");
        assert_eq!(reply_subject("Re: Hello"), "Re: Hello");
        assert_eq!(reply_subject("RE: Hello"), "RE: Hello");
        assert_eq!(reply_subject("re:Hello"), "re:Hello");
        assert_eq!(reply_subject("Fwd: Hello"), "Re: Fwd: Hello");
        assert_eq!(reply_subject("Ready"), "Re: Ready");
        assert_eq!(reply_subject(""), "Re:");
        assert_eq!(forward_subject("Hello"), "Fwd: Hello");
        assert_eq!(forward_subject("FW: Hello"), "FW: Hello");
        assert_eq!(forward_subject("Fwd: Hello"), "Fwd: Hello");
        assert_eq!(forward_subject("Re: Hello"), "Fwd: Re: Hello");
    }

    #[test]
    fn split_draft_from_gmail() {
        let quote = reply_quote_html(&source(), 0);
        let draft = format!("<div dir=\"ltr\">Thanks!<br></div><br>{quote}");
        assert_eq!(
            split_gmail_quote(&draft),
            (
                "<div dir=\"ltr\">Thanks!<br></div>".to_string(),
                Some(quote.clone())
            )
        );
        // Newer Gmail adds a second class; the editor part may be empty.
        let modern = "<div class=\"gmail_quote gmail_quote_container\">q</div>";
        assert_eq!(
            split_gmail_quote(modern),
            (String::new(), Some(modern.to_string()))
        );
        assert_eq!(
            split_gmail_quote("<div class=\"gmail_quote_container\">x</div>"),
            (
                "<div class=\"gmail_quote_container\">x</div>".to_string(),
                None
            )
        );
        assert_eq!(
            split_gmail_quote("<p>no quote</p>"),
            ("<p>no quote</p>".to_string(), None)
        );
    }

    #[test]
    fn history_markers() {
        let cases = [
            ("<div>Hi</div><br>", reply_quote_html(&source(), 0)),
            // Apple Mail: the split falls inside the wrapping div.
            (
                "<div>Hi</div><div><br>",
                "<blockquote type=\"cite\"><div>On Jan 6, Bob wrote:</div>old</blockquote></div>".to_string(),
            ),
            ("<p>Hi</p>", "<div class=\"moz-cite-prefix\">On 1/6 Bob wrote:</div><blockquote type=\"cite\">old</blockquote>".to_string()),
            ("<div>Hi</div>", "<div id=\"appendonsend\"></div><hr><div id=\"divRplyFwdMsg\"><b>From:</b> Bob</div>".to_string()),
            (
                "<div class=\"WordSection1\"><p>Hi</p>",
                "<div style=\"border:none;border-top:solid #E1E1E1 1.0pt;padding:3.0pt 0in 0in 0in\"><p><b>From:</b> Bob</p></div></div>".to_string(),
            ),
        ];
        for (visible, quoted) in cases {
            let html = format!("{visible}{quoted}");
            let expected_visible = visible.trim_end_matches("<br>").to_string();
            assert_eq!(
                split_quoted_history(&html),
                (expected_visible, Some(quoted)),
                "{html}"
            );
        }
    }

    #[test]
    fn history_not_split_when_nothing_above_or_forward() {
        let only_quote = reply_quote_html(&source(), 0);
        assert_eq!(
            split_quoted_history(&only_quote),
            (only_quote.clone(), None)
        );
        let forward = format!("<div>FYI</div>{}", forward_quote_html(&source(), 0));
        assert_eq!(split_quoted_history(&forward), (forward.clone(), None));
        assert_eq!(
            split_quoted_history("<p>plain</p>"),
            ("<p>plain</p>".to_string(), None)
        );
    }

    #[test]
    fn history_from_plain_text_quotes() {
        let html = crate::text_to_html("Yes.\n\nOn Mon, Bob wrote:\n> question?\n");
        let (visible, quoted) = split_quoted_history(&html);
        assert_eq!(
            visible,
            "<div dir=\"auto\" style=\"white-space:pre-wrap\">Yes."
        );
        assert!(quoted.unwrap().starts_with("<blockquote type=\"cite\""));
    }
}
