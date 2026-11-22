use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

use crate::collapse_whitespace;

#[must_use]
pub fn html_to_text(html: &str) -> String {
    collapse_whitespace(&render(html, Style::Search))
}

#[must_use]
pub fn snippet_text(snippet: &str) -> Cow<'_, str> {
    html_escape::decode_html_entities(snippet)
}

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
    out.truncate(out.trim_end_matches('\n').len());
    let leading = out.len() - out.trim_start_matches('\n').len();
    out.drain(..leading);
    out.push('\n');
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Search,
    Plain,
}

static COMMENT: LazyLock<Regex> =
    LazyLock::new(|| crate::regex(r"(?s)<!--.*?-->|<![^>]*>|<\?[^>]*>"));

static HIDDEN: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    ["head", "style", "script", "title", "noscript", "template"]
        .iter()
        .map(|tag| crate::regex(&format!(r"(?is)<{tag}\b[^>]*>.*?</{tag}\s*>")))
        .collect()
});

static TAG: LazyLock<Regex> = LazyLock::new(|| {
    crate::regex(r#"<(/?)([a-zA-Z][a-zA-Z0-9:-]*)((?:[^>=]|=\s*(?:"[^"]*"|'[^']*'|[^\s>]*))*)>"#)
});

static HREF: LazyLock<Regex> =
    LazyLock::new(|| crate::regex(r#"(?i)\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#));

fn render(html: &str, style: Style) -> String {
    let html = strip_hidden(html);
    let mut r = Render {
        w: TextWriter::new(style),
        pre_depth: 0,
        link: None,
    };
    let mut last = 0;
    for tag in TAG.captures_iter(&html) {
        let Some(whole) = tag.get(0) else { continue };
        r.w.text(
            html.get(last..whole.start()).unwrap_or_default(),
            r.pre_depth > 0,
        );
        last = whole.end();

        let is_end = tag.get(1).is_some_and(|m| !m.as_str().is_empty());
        let mut name = [0; MAX_TAG_NAME];
        let name = lowercase_tag_name(tag.get(2).map_or("", |m| m.as_str()), &mut name);
        let attributes = tag.get(3).map_or("", |m| m.as_str());
        r.tag(name, is_end, attributes);
    }
    r.w.text(html.get(last..).unwrap_or_default(), r.pre_depth > 0);
    r.w.out
}

fn strip_hidden(html: &str) -> Cow<'_, str> {
    let mut html = COMMENT.replace_all(html, "");
    for hidden in HIDDEN.iter() {
        if let Cow::Owned(replaced) = hidden.replace_all(&html, " ") {
            html = Cow::Owned(replaced);
        }
    }
    html
}

struct Render<'h> {
    w: TextWriter,
    pre_depth: u32,
    link: Option<(Cow<'h, str>, usize)>,
}

impl<'h> Render<'h> {
    fn tag(&mut self, name: &str, is_end: bool, attributes: &'h str) {
        let w = &mut self.w;
        let plain = w.style == Style::Plain;
        match (name, is_end) {
            ("br", _) => w.newline(),
            ("pre", false) => {
                self.pre_depth += 1;
                w.block();
            }
            ("pre", true) => {
                self.pre_depth = self.pre_depth.saturating_sub(1);
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
                if plain {
                    w.text("- ", true);
                }
            }
            (
                "div" | "li" | "tr" | "hr" | "dt" | "dd" | "section" | "article" | "header"
                | "footer" | "address" | "center",
                _,
            ) => w.block(),
            ("td" | "th", true) => w.text(" ", true),
            ("a", false) if plain => {
                self.link = href(attributes).map(|href| (href, w.out.len()));
            }
            ("a", true) if plain => self.close_link(),
            _ => {}
        }
    }

    fn close_link(&mut self) {
        let Some((href, start)) = self.link.take() else {
            return;
        };
        let shown = self.w.out.get(start..).unwrap_or_default().trim();
        let is_web = href.starts_with("http://") || href.starts_with("https://");
        if is_web && !shown.is_empty() && shown != href {
            self.w.text(&format!(" <{href}>"), true);
        }
    }
}

const MAX_TAG_NAME: usize = 10;

fn lowercase_tag_name<'b>(name: &str, buffer: &'b mut [u8; MAX_TAG_NAME]) -> &'b str {
    let Some(lowered) = buffer.get_mut(..name.len()) else {
        return "";
    };
    lowered.copy_from_slice(name.as_bytes());
    lowered.make_ascii_lowercase();
    std::str::from_utf8(lowered).unwrap_or_default()
}

fn href(attributes: &str) -> Option<Cow<'_, str>> {
    let found = HREF.captures(attributes)?;
    let value = found
        .get(1)
        .or_else(|| found.get(2))
        .or_else(|| found.get(3))?;
    Some(html_escape::decode_html_entities(value.as_str()))
}

const INVISIBLE: [char; 7] = [
    '\u{200b}', '\u{200c}', '\u{200d}', '\u{2060}', '\u{feff}', '\u{034f}', '\u{00ad}',
];

struct TextWriter {
    out: String,
    quote_depth: usize,
    style: Style,
}

impl TextWriter {
    const fn new(style: Style) -> Self {
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

    fn block(&mut self) {
        if !self.is_at_line_start() {
            self.newline();
        }
    }

    fn paragraph(&mut self) {
        self.block();
        if !self.out.is_empty() && !self.out.ends_with("\n\n") {
            self.newline();
        }
    }
}
