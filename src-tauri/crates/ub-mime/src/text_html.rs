//! Plain-text mail shown as HTML.

use std::fmt::Write as _;
use std::sync::LazyLock;

use regex::Regex;

/// Converts a plain-text body to HTML: escaped, newlines as `<br>`, spaces kept
/// with `white-space:pre-wrap`, and http(s) URLs linked. A trailing block of
/// `>`-quoted lines, with the `… wrote:` line before it, is wrapped in
/// `<blockquote type="cite">` so [`split_quoted_history`](crate::split_quoted_history)
/// can fold it. Sanitize the result like any other body.
pub fn text_to_html(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.trim_end().split('\n').collect();
    let mut out = String::from(r#"<div dir="auto" style="white-space:pre-wrap">"#);
    match trailing_quote_start(&lines) {
        Some(start) => {
            let (above, quoted) = lines.split_at(start);
            if !above.is_empty() {
                write_lines(&mut out, above);
                // A <br> just before a block element does not show; keep the break.
                out.push_str("<br>");
            }
            out.push_str(r#"<blockquote type="cite" style="margin:0">"#);
            write_lines(&mut out, quoted);
            out.push_str("</blockquote>");
        }
        None => write_lines(&mut out, &lines),
    }
    out.push_str("</div>");
    out
}

fn write_lines(out: &mut String, lines: &[&str]) {
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push_str("<br>");
        }
        write_linked(out, line);
    }
}

/// The index of the first line of the trailing run of `>`-quoted lines, or of
/// the `… wrote:` attribution line just before it.
fn trailing_quote_start(lines: &[&str]) -> Option<usize> {
    let mut start = None;
    for (i, line) in lines.iter().enumerate().rev() {
        let line = line.trim();
        if line.starts_with('>') {
            start = Some(i);
        } else if !line.is_empty() {
            break;
        }
    }
    let start = start?;
    let attribution = lines
        .get(..start)
        .unwrap_or_default()
        .iter()
        .rposition(|line| !line.trim().is_empty());
    match attribution {
        Some(i)
            if lines
                .get(i)
                .is_some_and(|l| l.trim_end().ends_with("wrote:")) =>
        {
            Some(i)
        }
        _ => Some(start),
    }
}

static URL: LazyLock<Regex> = LazyLock::new(|| crate::regex(r#"https?://[^\s<>"]+"#));

/// Writes one escaped line with its URLs as links. Trailing punctuation, and a
/// closing parenthesis the URL did not open, stay outside the link.
fn write_linked(out: &mut String, line: &str) {
    let mut last = 0;
    for found in URL.find_iter(line) {
        let url = trim_url(found.as_str());
        out.push_str(&html_escape::encode_text(
            line.get(last..found.start()).unwrap_or_default(),
        ));
        // Writing to a String cannot fail.
        let _ = write!(
            out,
            r#"<a href="{}">{}</a>"#,
            html_escape::encode_double_quoted_attribute(url),
            html_escape::encode_text(url)
        );
        last = found.start() + url.len();
    }
    out.push_str(&html_escape::encode_text(
        line.get(last..).unwrap_or_default(),
    ));
}

fn trim_url(mut url: &str) -> &str {
    loop {
        let mut trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
        if trimmed.ends_with(')') && trimmed.matches('(').count() < trimmed.matches(')').count() {
            trimmed = trimmed.get(..trimmed.len() - 1).unwrap_or_default();
        }
        if trimmed == url {
            return url;
        }
        url = trimmed;
    }
}
