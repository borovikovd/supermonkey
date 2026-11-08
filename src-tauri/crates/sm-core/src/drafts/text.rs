//! A draft's text as rows and rules read it.

use chrono::Offset;
use sm_mime::UtcOffset;

/// How many characters of a draft's text its list row and outbox row show.
const SNIPPET_CHARS: usize = 200;

/// A draft's body as plain text for its list row and send toast: the first
/// 200 characters, with `…` when there is more.
pub(crate) fn snippet(html: &str) -> String {
    let text = sm_mime::html_to_text(html);
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(SNIPPET_CHARS).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

/// Whether the editor's HTML holds nothing the user wrote: no text and no
/// image (SPEC UI state: the untouched rule).
pub(crate) fn is_blank(html: &str) -> bool {
    sm_mime::html_to_text(html).trim().is_empty() && !html.contains("<img")
}

/// Text as HTML that shows it as it is.
pub(crate) fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// The Mac's time zone now, for the dates quote blocks show.
pub(crate) fn local_offset() -> UtcOffset {
    UtcOffset(chrono::Local::now().offset().fix().local_minus_utc() / 60)
}
