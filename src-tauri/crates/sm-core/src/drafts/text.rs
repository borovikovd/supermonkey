use chrono::Offset;
use sm_mime::UtcOffset;

const SNIPPET_CHARS: usize = 200;

pub(crate) fn snippet(html: &str) -> String {
    let text = sm_mime::html_to_text(html);
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(SNIPPET_CHARS).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

pub(crate) fn is_blank(html: &str) -> bool {
    sm_mime::html_to_text(html).trim().is_empty() && !html.contains("<img")
}

pub(crate) fn local_offset() -> UtcOffset {
    UtcOffset(chrono::Local::now().offset().fix().local_minus_utc() / 60)
}
