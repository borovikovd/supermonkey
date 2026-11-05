//! HTML to text: one line of words for search and snippets, and a readable
//! `text/plain` alternative for outgoing mail.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

use crate::collapse_whitespace;

/// Plain text of an HTML body for full-text search and snippets: no `<head>`,
/// `<style>`, `<script>` or comments, entities decoded, and every run of
/// whitespace one space.
pub fn html_to_text(html: &str) -> String {
    collapse_whitespace(&render(html, Style::Search))
}

/// A Gmail `snippet` as text: Gmail sends it with HTML entities (`&#39;`,
/// `&amp;`) escaped, though it holds no markup.
pub fn snippet_text(snippet: &str) -> Cow<'_, str> {
    html_escape::decode_html_entities(snippet)
}

/// The `text/plain` alternative of an outgoing HTML body: lines from blocks and
/// `<br>`, `> ` before quoted lines, `- ` before list items, and `text <url>`
/// for http(s) links whose text is not the URL.
pub(crate) fn html_to_plain(html: &str) -> String {
    let text = render(html, Style::Plain);
    let mut out = String::with_capacity(text.len());
    let mut blank_lines = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.chars().all(|c| c == '>') {
            blank_lines += 1;
            if blank_lines > 1 {
                continue;
            }
        } else {
            blank_lines = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    let mut out = out.trim_matches('\n').to_owned();
    out.push('\n');
    out
}

/// What the text is for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    /// Words only; line structure does not matter.
    Search,
    /// Formatting meant for people reading the text part.
    Plain,
}

/// Comments, `<!DOCTYPE>` and `<![CDATA[` declarations, and `<?xml ?>`.
static COMMENT: LazyLock<Regex> =
    LazyLock::new(|| crate::regex(r"(?s)<!--.*?-->|<![^>]*>|<\?[^>]*>"));

/// Elements whose content is not text.
static HIDDEN: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    ["head", "style", "script", "title", "noscript", "template"]
        .iter()
        .map(|tag| crate::regex(&format!(r"(?is)<{tag}\b[^>]*>.*?</{tag}\s*>")))
        .collect()
});

/// A start or end tag: `/`, the name, and the attributes.
static TAG: LazyLock<Regex> = LazyLock::new(|| {
    crate::regex(r#"<(/?)([a-zA-Z][a-zA-Z0-9:-]*)((?:[^>"']|"[^"]*"|'[^']*')*)>"#)
});

static HREF: LazyLock<Regex> =
    LazyLock::new(|| crate::regex(r#"(?i)\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#));

/// Walks the tags of `html` and writes its text.
fn render(html: &str, style: Style) -> String {
    // Most bodies have few of these, so copy only when one matches.
    let mut html = COMMENT.replace_all(html, "");
    for hidden in HIDDEN.iter() {
        if let Cow::Owned(replaced) = hidden.replace_all(&html, " ") {
            html = Cow::Owned(replaced);
        }
    }

    let mut w = TextWriter::new(style);
    let mut pre_depth = 0u32;
    // The open link: its href and where its text starts in the output.
    let mut link: Option<(String, usize)> = None;
    let mut last = 0;
    for tag in TAG.captures_iter(&html) {
        let Some(whole) = tag.get(0) else { continue };
        w.text(
            html.get(last..whole.start()).unwrap_or_default(),
            pre_depth > 0,
        );
        last = whole.end();

        let is_end = tag.get(1).is_some_and(|m| !m.as_str().is_empty());
        let name = tag
            .get(2)
            .map_or_else(String::new, |m| m.as_str().to_ascii_lowercase());
        let attributes = tag.get(3).map_or("", |m| m.as_str());
        match (name.as_str(), is_end) {
            ("br", _) => w.newline(),
            ("pre", false) => {
                pre_depth += 1;
                w.block();
            }
            ("pre", true) => {
                pre_depth = pre_depth.saturating_sub(1);
                w.block();
            }
            ("blockquote", false) => {
                w.block();
                w.quote_depth += 1;
            }
            ("blockquote", true) => {
                w.block();
                w.quote_depth = w.quote_depth.saturating_sub(1);
            }
            ("p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "table", _) => {
                w.paragraph();
            }
            ("li", false) => {
                w.block();
                if style == Style::Plain {
                    w.text("- ", true);
                }
            }
            (
                "div" | "li" | "tr" | "hr" | "dt" | "dd" | "section" | "article" | "header"
                | "footer" | "address" | "center",
                _,
            ) => w.block(),
            ("td" | "th", true) => w.text(" ", true),
            ("a", false) if style == Style::Plain => {
                link = href(attributes).map(|href| (href, w.out.len()));
            }
            ("a", true) if style == Style::Plain => {
                if let Some((href, start)) = link.take() {
                    // `start` can be past the end after a newline trimmed spaces.
                    let shown = w.out.get(start..).unwrap_or_default().trim();
                    let is_web = href.starts_with("http://") || href.starts_with("https://");
                    if is_web && !shown.is_empty() && shown != href {
                        w.text(&format!(" <{href}>"), true);
                    }
                }
            }
            _ => {}
        }
    }
    w.text(html.get(last..).unwrap_or_default(), pre_depth > 0);
    w.out
}

/// The decoded `href` of a start tag's attributes.
fn href(attributes: &str) -> Option<String> {
    let found = HREF.captures(attributes)?;
    let value = found
        .get(1)
        .or_else(|| found.get(2))
        .or_else(|| found.get(3))?;
    Some(html_escape::decode_html_entities(value.as_str()).into_owned())
}

/// Zero-width and filler characters newsletters pad their preview text with.
const INVISIBLE: [char; 7] = [
    '\u{200b}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{feff}', '\u{034f}', '\u{00ad}',
];

/// Accumulates text, keeping line structure and quote prefixes.
struct TextWriter {
    out: String,
    quote_depth: usize,
    style: Style,
}

impl TextWriter {
    fn new(style: Style) -> Self {
        Self {
            out: String::new(),
            quote_depth: 0,
            style,
        }
    }

    fn is_at_line_start(&self) -> bool {
        self.out.is_empty() || self.out.ends_with('\n')
    }

    fn quote_prefix(&mut self) {
        if self.style == Style::Plain && self.quote_depth > 0 {
            self.out.push_str(&"> ".repeat(self.quote_depth));
        }
    }

    /// Writes text from between tags. Outside `<pre>`, a whitespace run is one
    /// space, never at a line start and never doubled.
    fn text(&mut self, raw: &str, is_preformatted: bool) {
        let decoded: String = html_escape::decode_html_entities(raw)
            .chars()
            .filter(|c| !INVISIBLE.contains(c))
            .map(|c| if c == '\u{a0}' { ' ' } else { c })
            .collect();
        if is_preformatted {
            for (i, line) in decoded.split('\n').enumerate() {
                if i > 0 {
                    self.newline();
                }
                if !line.is_empty() {
                    if self.is_at_line_start() {
                        self.quote_prefix();
                    }
                    self.out.push_str(line);
                }
            }
            return;
        }
        let collapsed = collapse_whitespace(&decoded);
        let is_at_line_start = self.is_at_line_start();
        if decoded.starts_with(char::is_whitespace) && !is_at_line_start && !self.out.ends_with(' ')
        {
            self.out.push(' ');
        }
        if collapsed.is_empty() {
            return;
        }
        if is_at_line_start {
            self.quote_prefix();
        }
        self.out.push_str(&collapsed);
        if decoded.ends_with(char::is_whitespace) {
            self.out.push(' ');
        }
    }

    fn newline(&mut self) {
        if self.is_at_line_start() && self.style == Style::Plain && self.quote_depth > 0 {
            self.out.push_str(&">".repeat(self.quote_depth));
        }
        let trimmed = self.out.trim_end_matches(' ').len();
        self.out.truncate(trimmed);
        self.out.push('\n');
    }

    /// Ends the current line, if one is open.
    fn block(&mut self) {
        if !self.is_at_line_start() {
            self.newline();
        }
    }

    /// Ends the current line and leaves one blank line.
    fn paragraph(&mut self) {
        self.block();
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.newline();
        }
    }
}
